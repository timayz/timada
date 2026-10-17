//! Takes a catalogue snapshot from a WooCommerce shop:
//!
//! ```sh
//! woo-snapshot --site https://www.chatounette.com --out examples/chatounette/seed/catalog.json.gz
//! ```

use std::path::PathBuf;

use clap::Parser;
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
    /// Where to write the snapshot; gzipped when it ends in .gz
    #[arg(long)]
    out: PathBuf,
    /// Top-level category slugs in the order the menu shows them, comma-separated
    #[arg(long, value_delimiter = ',')]
    menu_order: Vec<String>,
    /// Category slugs left out of the menu, comma-separated
    #[arg(long, value_delimiter = ',')]
    hide: Vec<String>,
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
    };
    let snapshot = snapshot_site(&client, &cli.site, &options).await?;
    snapshot.write(&cli.out)?;
    tracing::info!(
        path = %cli.out.display(),
        categories = snapshot.categories.len(),
        products = snapshot.products.len(),
        variants = snapshot.products.iter().map(|p| p.variants.len()).sum::<usize>(),
        pictures = snapshot.products.iter().map(|p| p.images.len()).sum::<usize>(),
        "snapshot written"
    );
    Ok(())
}
