//! `seed`: fills a tenant with a catalogue snapshot, with the server
//! stopped. Makes the tenant when it is missing, loads the categories and
//! products through the catalog's commands (copying their pictures from the
//! snapshot into this node's media directory, or fetching from the shop the
//! ones the snapshot lacks), gives the shop one delivery method and a
//! promo code so a storefront can sell right away, then mints a key for
//! that storefront and prints its settings.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use timada_catalog::{MAX_IMAGE_BYTES, new_image_id};
use timada_catalog_import::{BoxFuture, Images, ImportOptions, NoImages, Snapshot, StoredImage};
use timada_tenancy::{CreateApiKey, CreateTenant, SCOPE_STORE_CHECKOUT, SCOPE_STORE_READ};

use crate::cluster::{Cluster, Executor};
use crate::config::Config;
use crate::db;
use crate::media::Media;
use crate::tenants::Tenants;

#[derive(Args, Debug)]
pub struct SeedArgs {
    /// The catalogue to load: a snapshot directory (catalog.json.gz and its
    /// pictures), or a catalogue file alone
    #[arg(long)]
    pub snapshot: PathBuf,
    /// The tenant's name; created when no tenant has its slug
    #[arg(long)]
    pub tenant: String,
    /// The tenant's URL handle; derived from the name when omitted
    #[arg(long)]
    pub slug: Option<String>,
    /// E-mail of a registered user, who owns the tenant when it is created
    #[arg(long)]
    pub owner: String,
    /// Load at most this many products
    #[arg(long)]
    pub limit: Option<usize>,
    /// Leave the pictures out
    #[arg(long)]
    pub skip_images: bool,
    /// Pictures copied or fetched at once
    #[arg(long, default_value_t = 8)]
    pub concurrency: usize,
    /// Where the shop's storefront is reached, as its mails link to it
    #[arg(long)]
    pub storefront_url: Option<String>,
    /// Do not mint a storefront key
    #[arg(long)]
    pub no_key: bool,
}

/// The delivery method and the code the seeded shop opens with.
const SHIPPING_METHOD: &str = "Livraison avec suivi";
const SHIPPING_CENTS: u64 = 490;
const FREE_ABOVE_CENTS: u64 = 6_500;
const SHIPPING_COUNTRIES: &[&str] = &["FR", "BE", "CH"];
const PROMO_CODE: &str = "MONSTER10";
/// Basis points: 10 %.
const PROMO_PERCENT: u32 = 1_000;

pub async fn seed(config: Config, args: SeedArgs) -> Result<()> {
    refuse_if_serving(&config).await?;
    let snapshot = Snapshot::read(&args.snapshot)?;
    let pictures = snapshot.images().count();
    tracing::info!(
        source = %snapshot.source,
        categories = snapshot.categories.len(),
        products = snapshot.products.len(),
        pictures,
        pictures_in_snapshot = snapshot.images().filter(|i| i.file.is_some()).count(),
        "snapshot read"
    );
    super::migrate(config.clone()).await?;

    let write_pool = db::create_write_pool(&config.database.url).await?;
    let read_pool =
        db::create_read_pool(&config.database.url, config.database.max_connections).await?;
    let cluster = Arc::new(Cluster::build(config.cluster.as_ref()).await?);
    let platform = cluster
        .open_platform(
            config.server.region.as_deref(),
            read_pool.clone(),
            write_pool.clone(),
        )
        .await?;
    let peers = crate::peers::Peers::from_cluster(config.cluster.as_ref())?.map(Arc::new);
    let media = Arc::new(Media::new(&config.media, peers));

    // The owner, by the address they registered with.
    let identity = timada_identity::Command(platform.clone());
    let email = timada_identity::normalize_email(&args.owner)
        .map_err(|_| anyhow::anyhow!("{:?} is not an e-mail address", args.owner))?;
    let account = identity
        .load_email_account(&email)
        .await?
        .with_context(|| format!("no user is registered with {email}: register first"))?;
    let owner = identity
        .load_user(&account.user_id)
        .await?
        .context("the owner's account was deleted")?;

    // The tenant, made unless it is there.
    let tenancy = timada_tenancy::Command(platform.clone());
    let slug = args
        .slug
        .clone()
        .unwrap_or_else(|| timada_tenancy::slugify(&args.tenant));
    let tenant = match tenancy.load_tenant_by_slug(&slug).await? {
        Some(tenant) => {
            ensure!(!tenant.is_suspended(), "tenant {slug} is suspended");
            ensure!(
                tenant.members.contains_key(&owner.id),
                "{email} is not a member of tenant {slug}"
            );
            tracing::info!(%slug, "tenant exists");
            tenant
        }
        None => {
            let tenant = tenancy
                .create_tenant(CreateTenant {
                    name: args.tenant.clone(),
                    owner_user_id: owner.id.clone(),
                })
                .await?;
            ensure!(
                tenant.slug == slug,
                "the tenant got slug {} instead of {slug}: pass --slug",
                tenant.slug
            );
            tracing::info!(%slug, id = %tenant.id, "tenant created");
            tenant
        }
    };
    // So the back office lists it right away.
    timada_tenancy::read_model_subscription::<Executor>(write_pool.clone())
        .run_once(&platform)
        .await?;

    let tenants = Tenants::new(
        cluster.clone(),
        &config.tenants,
        config.server.region.clone(),
        platform.clone(),
        write_pool.clone(),
        vec![],
    );
    let hold = tenants.hold(&tenant).await?;
    let executor = hold.executor().clone();

    let images: Arc<dyn Images> = if args.skip_images {
        Arc::new(NoImages)
    } else {
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        Arc::new(MediaImages {
            client: reqwest::Client::builder()
                .user_agent("timada-dashboard seed")
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            media: media.clone(),
            tenant_id: tenant.id.clone(),
        })
    };
    let report = timada_catalog_import::import(
        &timada_catalog::Command(executor.clone()),
        &owner.id,
        &snapshot,
        images,
        &ImportOptions {
            limit: args.limit,
            concurrency: args.concurrency,
        },
    )
    .await?;
    for warning in &report.warnings {
        tracing::warn!("{warning}");
    }
    tracing::info!(
        categories_created = report.categories_created,
        categories_skipped = report.categories_skipped,
        products_created = report.products_created,
        products_skipped = report.products_skipped,
        variants = report.variants,
        pictures = report.images,
        warnings = report.warnings.len(),
        "catalogue loaded"
    );

    open_shop(&executor, &owner.id, args.storefront_url.as_deref()).await?;

    let key = if args.no_key {
        None
    } else {
        Some(
            tenancy
                .create_api_key(
                    &owner.id,
                    CreateApiKey {
                        tenant_id: tenant.id.clone(),
                        label: "Storefront (seed)".into(),
                        scopes: vec![SCOPE_STORE_READ.into(), SCOPE_STORE_CHECKOUT.into()],
                    },
                    crate::auth::now(),
                )
                .await?,
        )
    };

    drop(hold);
    tenants.shutdown().await;
    read_pool.close().await;
    write_pool.close().await;

    if let Some(key) = key {
        // The command's answer, for the shell that runs the storefront.
        println!("STORE_TOKEN={}", key.token);
        println!("STORE_MEDIA_URL={}", media.tenant_url(&tenant.id));
        println!("STORE_API_URL={}", config.server.url.trim_end_matches('/'));
    }
    Ok(())
}

/// Two processes on one node's files would each run their own consensus
/// over the same journal: the seed runs only while the server is stopped,
/// which the server's port tells.
async fn refuse_if_serving(config: &Config) -> Result<()> {
    let host = config.server.host.as_str();
    let port = config.server.port;
    match tokio::net::TcpListener::bind((host, port)).await {
        Ok(probe) => drop(probe),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            bail!(
                "{host}:{port} is in use: stop the server (`timada-dashboard serve`) before seeding"
            )
        }
        Err(err) => return Err(err).with_context(|| format!("cannot probe {host}:{port}")),
    }
    Ok(())
}

/// What a storefront needs to sell what was loaded: a way to deliver, the
/// address it is reached at and the code the shop advertises. Each is
/// left alone when the shop already has it.
async fn open_shop(executor: &Executor, actor: &str, storefront_url: Option<&str>) -> Result<()> {
    let orders = timada_orders::Command(executor.clone());
    let has_method = orders
        .load_shipping_methods()
        .await?
        .is_some_and(|state| state.methods.iter().any(|m| m.name == SHIPPING_METHOD));
    if !has_method {
        orders
            .add_shipping_method(
                actor,
                timada_orders::NewShippingMethod {
                    name: SHIPPING_METHOD.into(),
                    price_cents: SHIPPING_CENTS,
                    currency: "EUR".into(),
                    countries: SHIPPING_COUNTRIES.iter().map(|c| (*c).to_owned()).collect(),
                    free_above_cents: Some(FREE_ABOVE_CENTS),
                },
            )
            .await?;
        tracing::info!(method = SHIPPING_METHOD, "delivery method added");
    }
    if let Some(url) = storefront_url {
        orders.set_storefront_address(actor, Some(url)).await?;
    }
    let discounts = timada_discounts::Command(executor.clone());
    if discounts.load_discount_by_code(PROMO_CODE).await?.is_none() {
        discounts
            .create_discount(
                actor,
                timada_discounts::CreateDiscount {
                    code: PROMO_CODE.into(),
                    value: timada_discounts::DiscountValue::Percent(PROMO_PERCENT),
                    starts_at: None,
                    ends_at: None,
                    max_uses: None,
                    once_per_email: false,
                },
            )
            .await?;
        tracing::info!(code = PROMO_CODE, "promo code added");
    }
    Ok(())
}

/// Pictures from the snapshot's files, or fetched from the shop it came
/// from, kept where this node serves them (and on a majority of the
/// cluster, like an upload).
struct MediaImages {
    client: reqwest::Client,
    media: Arc<Media>,
    tenant_id: String,
}

impl MediaImages {
    /// Checks the bytes are a picture the shop takes, then stores them
    /// under a fresh id; `what` names them in the log.
    async fn keep(&self, bytes: bytes::Bytes, what: &str) -> anyhow::Result<Option<StoredImage>> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            tracing::warn!(
                what,
                size = bytes.len(),
                "picture empty or too large, left out"
            );
            return Ok(None);
        }
        let Some(sniffed) = Media::sniff(&bytes) else {
            tracing::warn!(what, "not a picture the shop accepts, left out");
            return Ok(None);
        };
        let image_id = new_image_id();
        let file = format!("{image_id}.{}", sniffed.ext);
        self.media
            .store(&self.tenant_id, &image_id, sniffed.ext, &bytes)
            .await?;
        if let Err(err) = self
            .media
            .replicate(&self.tenant_id, &file, bytes.to_vec())
            .await
        {
            self.media.remove(&self.tenant_id, &file).await;
            return Err(err);
        }
        Ok(Some(StoredImage {
            image_id,
            ext: sniffed.ext.to_owned(),
            size: bytes.len() as u64,
            width: sniffed.width,
            height: sniffed.height,
        }))
    }
}

impl Images for MediaImages {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, anyhow::Result<Option<StoredImage>>> {
        Box::pin(async move {
            let response = self.client.get(url).send().await?.error_for_status()?;
            if response
                .content_length()
                .is_some_and(|length| length > MAX_IMAGE_BYTES)
            {
                tracing::warn!(url, "picture too large, left out");
                return Ok(None);
            }
            let bytes = response.bytes().await?;
            self.keep(bytes, url).await
        })
    }

    fn store<'a>(&'a self, bytes: Vec<u8>) -> BoxFuture<'a, anyhow::Result<Option<StoredImage>>> {
        Box::pin(async move { self.keep(bytes::Bytes::from(bytes), "snapshot").await })
    }

    fn discard<'a>(&'a self, image: &'a StoredImage) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.media
                .remove(
                    &self.tenant_id,
                    &format!("{}.{}", image.image_id, image.ext),
                )
                .await;
        })
    }
}
