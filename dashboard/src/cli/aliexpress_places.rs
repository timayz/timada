//! `aliexpress-places`: reads AliExpress's lists of places into the file the
//! build embeds, one call per country with the app's key and no buyer's.
//! A list that does not answer stops it with the file as it was, so a bad day
//! at AliExpress never ships as a country without a list.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use timada_aliexpress::{Client, DEFAULT_API_URL, PLACE_COUNTRIES, encode_places, places_of};

use crate::config::Config;

/// Between two countries: nothing here is in a hurry.
const PAUSE: Duration = Duration::from_secs(1);

#[derive(clap::Args)]
pub struct AliexpressPlacesArgs {
    /// Where to write the lists
    #[arg(long, default_value = "crates/aliexpress/places.bin")]
    pub out: PathBuf,
}

pub async fn aliexpress_places(config: Config, args: AliexpressPlacesArgs) -> Result<()> {
    let aliexpress = &config.aliexpress;
    let (Some(key), Some(secret)) = (&aliexpress.app_key, &aliexpress.app_secret) else {
        anyhow::bail!("reading the lists needs the app's key ([aliexpress] app_key, app_secret)");
    };
    let api_url = aliexpress.api_url.as_deref().unwrap_or(DEFAULT_API_URL);
    let http = reqwest::Client::builder()
        .user_agent("timada")
        .timeout(Duration::from_secs(20))
        .build()?;
    let client = Client::new(http, api_url, key.clone(), secret.clone());

    let mut lists = Vec::new();
    for country in PLACE_COUNTRIES {
        let tree = client
            .address_tree(country)
            .await
            .with_context(|| format!("{country}'s list not read: the file is as it was"))?;
        let before = places_of(country).map_or(0, |tree| tree.len());
        match tree {
            Some(tree) => {
                tracing::info!(country, places = tree.len(), before, "list read");
                lists.push(((*country).to_owned(), tree));
            }
            None if before > 0 => {
                tracing::warn!(country, before, "this country no longer has a list");
            }
            None => tracing::info!(country, "no list"),
        }
        tokio::time::sleep(PAUSE).await;
    }

    let file = encode_places(&lists)?;
    let written = args.out.with_extension("bin.new");
    std::fs::write(&written, &file)
        .with_context(|| format!("cannot write {}", written.display()))?;
    std::fs::rename(&written, &args.out)
        .with_context(|| format!("cannot write {}", args.out.display()))?;
    tracing::info!(
        out = %args.out.display(),
        countries = lists.len(),
        bytes = file.len(),
        "lists written: build again to ship them"
    );
    Ok(())
}
