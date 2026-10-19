//! Takes a catalogue snapshot from a WooCommerce shop into a directory:
//! `catalog.json.gz` and, under `pictures/`, every picture it names, so a
//! seed from it needs no network. Taken again into the same directory, only
//! the pictures not there yet are fetched.
//!
//! ```sh
//! woo-snapshot --site https://www.chatounette.com --out seeds/chatounette
//! ```

use std::path::PathBuf;

use clap::Parser;
use timada_catalog_import::CATALOG_FILE;
use timada_catalog_import::woocommerce::{SiteOptions, snapshot_site};

#[derive(Parser)]
#[command(
    name = "woo-snapshot",
    about = "Snapshots a WooCommerce shop's catalogue"
)]
struct Cli {
    /// The shop's address, e.g. https://www.chatounette.com
    #[arg(long)]
    site: String,
    /// The snapshot directory to write (catalog.json.gz and pictures/)
    #[arg(long)]
    out: PathBuf,
    /// Leave the pictures to the seed to fetch
    #[arg(long)]
    no_pictures: bool,
    /// Pictures fetched at once
    #[arg(long, default_value_t = 8)]
    concurrency: usize,
    /// Shop pages read at once (a shop's pages are slower and more guarded
    /// than its pictures)
    #[arg(long, default_value_t = 2)]
    page_concurrency: usize,
    /// Top-level category slugs in the order the menu shows them, comma-separated
    #[arg(long, value_delimiter = ',')]
    menu_order: Vec<String>,
    /// Category slugs left out of the menu, comma-separated
    #[arg(long, value_delimiter = ',')]
    hide: Vec<String>,
    /// Headings of a product's "Description" tab where the shop's own
    /// blocks begin, comma-separated; the tab is read up to the first one
    #[arg(long, value_delimiter = ',')]
    tab_stop_at: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent("timada-catalog-import/0.1 (+https://github.com/timayz/timada)")
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let options = SiteOptions {
        menu_order: cli.menu_order,
        hidden: cli.hide,
        tab_stop_at: cli.tab_stop_at,
    };
    let mut snapshot = snapshot_site(&client, &cli.site, &options, cli.page_concurrency).await?;
    // The catalogue first, so a fetch of the pictures that stops leaves one.
    let catalog = cli.out.join(CATALOG_FILE);
    snapshot.write(&catalog)?;
    tracing::info!(
        path = %catalog.display(),
        categories = snapshot.categories.len(),
        products = snapshot.products.len(),
        variants = snapshot.products.iter().map(|p| p.variants.len()).sum::<usize>(),
        pictures = snapshot.images().count(),
        "catalogue written"
    );
    if !cli.no_pictures {
        let report = snapshot
            .fetch_pictures(&client, &cli.out, cli.concurrency)
            .await?;
        snapshot.write(&catalog)?;
        tracing::info!(
            kept = report.kept,
            fetched = report.fetched,
            failed = report.failed.len(),
            "pictures written"
        );
    }
    Ok(())
}
