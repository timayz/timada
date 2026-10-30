//! `seed`: fills a tenant with a catalogue snapshot, with the server
//! stopped. Makes the tenant when it is missing, loads the categories and
//! products through the catalog's commands (copying their pictures from the
//! snapshot into this node's media directory, or fetching from the shop the
//! ones the snapshot lacks), gives the shop one delivery method and a
//! promo code so a storefront can sell right away, writes and publishes the
//! shop's own pages from a directory of Markdown files when given one, then
//! mints a key for that storefront and prints its settings.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use timada_catalog_import::{Images, ImportOptions, NoImages, Snapshot};
use timada_tenancy::{CreateApiKey, CreateTenant, SCOPE_STORE_CHECKOUT, SCOPE_STORE_READ};

use crate::cluster::{Cluster, Executor, Files, StorePools};
use crate::config::Config;
use crate::db;
use crate::media::Media;
use crate::media_images::MediaImages;

#[derive(Args, Debug)]
pub struct SeedArgs {
    /// The catalogue to load: a snapshot directory (catalog.json.gz and its
    /// pictures)
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
    /// A directory of the shop's own pages, one Markdown file each, published
    /// under the file's name (see `read_pages`); a page whose address is
    /// taken already is left alone
    #[arg(long)]
    pub pages: Option<PathBuf>,
    /// Registers the owner with this password when no user has their
    /// address yet, without mailing a code: for a throwaway platform (the
    /// load test's), never a real one
    #[arg(long)]
    pub owner_password: Option<String>,
    /// Makes the shop sell for pretend: a test company and the Stripe test
    /// keys of `TEST_PAYMENTS`, for a stand-in for Stripe (the load test's)
    #[arg(long)]
    pub test_payments: bool,
    /// Also write the storefront's settings it prints to this file, for a
    /// script, or a snapshot to keep (`snapshot save --also`)
    #[arg(long)]
    pub env_file: Option<PathBuf>,
}

/// The delivery method and the code the seeded shop opens with.
const SHIPPING_METHOD: &str = "Livraison avec suivi";
const SHIPPING_CENTS: u64 = 490;
const FREE_ABOVE_CENTS: u64 = 6_500;
const SHIPPING_COUNTRIES: &[&str] = &["FR", "BE", "CH"];
const PROMO_CODE: &str = "MONSTER10";
/// Basis points: 10 %.
const PROMO_PERCENT: u32 = 1_000;
/// How long the seed waits for the other nodes of a cluster to answer.
const PEERS_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
/// The keys `--test-payments` connects: what the stand-in for Stripe the
/// load test runs signs its webhooks with.
const TEST_PUBLISHABLE_KEY: &str = "pk_test_load";
const TEST_SECRET_KEY: &str = "sk_test_load";
const TEST_WEBHOOK_SECRET: &str = "whsec_load";

pub async fn seed(config: Config, args: SeedArgs) -> Result<()> {
    super::refuse_if_serving(&config).await?;
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

    let files = Files {
        store_url: config.database.store.clone(),
        read_models_url: config.database.read_models.clone(),
    };
    let store_write =
        db::create_write_pool(&config.database.store, db::Durability::Durable).await?;
    let store_cursors =
        db::create_write_pool(&config.database.store, db::Durability::Relaxed).await?;
    let store_read =
        db::create_read_pool(&config.database.store, config.database.max_connections).await?;
    let (cluster, platform) = Cluster::open(
        config.cluster.as_ref(),
        config.server.region.as_deref(),
        &files,
        StorePools {
            read: store_read.clone(),
            write: store_write.clone(),
            cursors: store_cursors.clone(),
        },
    )
    .await?;
    let write_pool =
        db::create_write_pool(&config.database.read_models, db::Durability::Durable).await?;
    let read_pool = db::create_read_pool(
        &config.database.read_models,
        config.database.max_connections,
    )
    .await?;
    let peers = crate::peers::Peers::from_cluster(config.cluster.as_ref())?.map(Arc::new);
    let media = Arc::new(Media::new(&config.media, peers));

    cluster.until_confirmed(PEERS_WAIT).await?;

    // The owner, by the address they registered with.
    let identity =
        timada_identity::Command::new(platform.clone(), crate::seal::email_codes(&config).0);
    let email = timada_identity::normalize_email(&args.owner)
        .map_err(|_| anyhow::anyhow!("{:?} is not an e-mail address", args.owner))?;
    // The first read waits for a quorum, which a node just opened may not
    // reach until it hears from its peers.
    let started = tokio::time::Instant::now();
    let registered = loop {
        match identity.load_email_account(&email).await {
            Ok(account) => break account.is_some(),
            Err(err) if started.elapsed() < PEERS_WAIT => {
                tracing::info!(%err, "waiting for the other nodes");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(err) => return Err(err),
        }
    };
    if let Some(password) = &args.owner_password
        && !registered
    {
        register_owner(&identity, &email, password).await?;
    }
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

    let executor = platform.clone();

    let images: Arc<dyn Images> = if args.skip_images {
        Arc::new(NoImages)
    } else {
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        // The operator names the snapshot: its addresses may be anywhere.
        Arc::new(MediaImages::new(
            reqwest::Client::builder()
                .user_agent("timada-dashboard seed")
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            media.clone(),
            tenant.id.clone(),
            false,
        ))
    };
    let report = timada_catalog_import::import(
        &timada_catalog::Command::new(executor.clone(), &tenant.id),
        &owner.id,
        &snapshot,
        images,
        &ImportOptions {
            limit: args.limit,
            concurrency: args.concurrency,
            ..Default::default()
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

    open_shop(
        &executor,
        &tenant.id,
        &owner.id,
        args.storefront_url.as_deref(),
    )
    .await?;
    // So the catalogue pages show the products without waiting for `serve`.
    timada_catalog::read_model_subscription::<Executor>(write_pool.clone())
        .run_once(&platform)
        .await?;

    if args.test_payments {
        test_payments(&config, &tenancy, &executor, &tenant.id, &owner.id).await?;
    }

    if let Some(dir) = &args.pages {
        let pages = timada_pages::Command::new(executor.clone(), &tenant.id);
        let (created, skipped) = seed_pages(&pages, &owner.id, dir).await?;
        tracing::info!(created, skipped, "pages loaded");
        timada_pages::read_model_subscription::<Executor>(write_pool.clone())
            .run_once(&platform)
            .await?;
    }

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

    cluster.shutdown().await;
    read_pool.close().await;
    write_pool.close().await;
    store_read.close().await;
    store_cursors.close().await;
    store_write.close().await;

    let mut settings = Vec::new();
    if let Some(key) = key {
        settings.push(format!("STORE_TOKEN={}", key.token));
        settings.push(format!("STORE_MEDIA_URL={}", media.tenant_url(&tenant.id)));
        settings.push(format!(
            "STORE_API_URL={}",
            config.server.url.trim_end_matches('/')
        ));
    }
    if args.test_payments {
        settings.push(format!("STORE_TENANT_ID={}", tenant.id));
    }
    // The command's answer, for the shell that runs the storefront.
    for line in &settings {
        println!("{line}");
    }
    if let Some(file) = &args.env_file
        && !settings.is_empty()
    {
        std::fs::write(file, settings.join("\n") + "\n")
            .with_context(|| format!("cannot write {}", file.display()))?;
    }
    Ok(())
}

/// Registers `email` as a user with `password`, issuing and burning the
/// registration code in place of the mail that would carry it.
async fn register_owner(
    identity: &timada_identity::Command<Executor>,
    email: &str,
    password: &str,
) -> Result<()> {
    let now = crate::auth::now();
    let issued = identity
        .issue_email_code(email, timada_identity::ChallengePurpose::Register, now)
        .await?;
    identity
        .register_with_password(
            timada_identity::RegisterWithPassword {
                email: issued.email,
                code: issued.code,
                password_hash: timada_identity::hash_password(password).await?,
                context: timada_identity::LoginContext::default(),
            },
            now,
        )
        .await?;
    tracing::info!(%email, "owner registered");
    Ok(())
}

/// The test company the shop sells as, registered once, and the Stripe test
/// keys, sealed as the payments page seals them; then the setup the
/// storefront reads, as the payments subscription would write it.
async fn test_payments(
    config: &Config,
    tenancy: &timada_tenancy::Command<Executor>,
    executor: &Executor,
    tenant_id: &str,
    owner_id: &str,
) -> Result<()> {
    let sealer = crate::seal::payments_sealer(config)
        .context("--test-payments needs payments enabled (a [payments] section)")?;
    let identity = timada_tenancy::LegalIdentity {
        legal_name: "Load Test SAS".into(),
        legal_form: "SAS".into(),
        siret: "12002701600092".into(),
        line1: "1 rue de la Paix".into(),
        postal_code: "75002".into(),
        city: "Paris".into(),
        country: "FR".into(),
        contact_email: "shop@example.org".into(),
        vat_number: Some("FR85120027016".into()),
        ..Default::default()
    };
    let tenant = tenancy
        .load_tenant(tenant_id)
        .await?
        .context("the tenant vanished")?;
    if tenant.company.is_none() {
        let company_id = match tenancy.load_company_siren(identity.siren()).await? {
            Some(siren) if !siren.released => siren.company_id,
            _ => tenancy.create_company(owner_id, identity).await?.id,
        };
        tenancy
            .link_tenant(owner_id, &company_id, tenant_id, None)
            .await?;
        tracing::info!(%company_id, "test company linked");
    }
    tenancy
        .configure_payments(
            owner_id,
            tenant_id,
            timada_tenancy::ConfigurePayments {
                provider: timada_orders::PROVIDER_STRIPE.into(),
                publishable_key: TEST_PUBLISHABLE_KEY.into(),
                secret_key_sealed: sealer.seal(TEST_SECRET_KEY)?,
                webhook_secret_sealed: sealer.seal(TEST_WEBHOOK_SECRET)?,
            },
        )
        .await?;
    timada_orders::Command::new(executor.clone(), tenant_id)
        .set_payment_method(
            crate::payments::DASHBOARD_ACTOR,
            Some(timada_orders::PaymentSetup {
                provider: timada_orders::PROVIDER_STRIPE.into(),
                publishable_key: TEST_PUBLISHABLE_KEY.into(),
            }),
        )
        .await?;
    tracing::info!("test payments connected");
    Ok(())
}

/// What a storefront needs to sell what was loaded: a way to deliver, the
/// address it is reached at and the code the shop advertises. Each is
/// left alone when the shop already has it.
async fn open_shop(
    executor: &Executor,
    tenant_id: &str,
    actor: &str,
    storefront_url: Option<&str>,
) -> Result<()> {
    let orders = timada_orders::Command::new(executor.clone(), tenant_id);
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
    let discounts = timada_discounts::Command::new(executor.clone(), tenant_id);
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

/// One page of a `--pages` directory.
#[derive(Debug, Default, PartialEq, Eq)]
struct SeedPage {
    slug: String,
    title: String,
    meta_title: String,
    meta_description: String,
    body: String,
}

/// Reads the pages of a directory: each `{slug}.md` opens with a header
/// between `---` lines of `key: value` (`title`, required; `meta_title`,
/// and `meta_description`), and the
/// Markdown body follows. In file-name order.
fn read_pages(dir: &std::path::Path) -> Result<Vec<SeedPage>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let slug = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            parse_page(slug, &text).with_context(|| format!("in {}", path.display()))
        })
        .collect()
}

fn parse_page(slug: String, text: &str) -> Result<SeedPage> {
    let Some(rest) = text.strip_prefix("---\n") else {
        bail!("a page opens with a `---` header");
    };
    let Some((header, body)) = rest.split_once("\n---\n") else {
        bail!("the header ends with a `---` line");
    };
    let mut page = SeedPage {
        slug,
        body: body.trim().to_owned(),
        ..Default::default()
    };
    for line in header.lines().filter(|line| !line.trim().is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            bail!("{line:?} is not `key: value`");
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "title" => page.title = value,
            "meta_title" => page.meta_title = value,
            "meta_description" => page.meta_description = value,
            other => bail!("unknown header {other:?}"),
        }
    }
    ensure!(!page.title.is_empty(), "a page has a title");
    Ok(page)
}

/// Writes and publishes the pages of `dir`, leaving alone those whose
/// address is taken: how many it created, and skipped.
async fn seed_pages(
    pages: &timada_pages::Command<Executor>,
    actor: &str,
    dir: &std::path::Path,
) -> Result<(usize, usize)> {
    let (mut created, mut skipped) = (0, 0);
    for page in read_pages(dir)? {
        if pages.load_page_by_slug(&page.slug).await?.is_some() {
            skipped += 1;
            continue;
        }
        let id = pages
            .create_page(
                actor,
                timada_pages::CreatePage {
                    slug: page.slug.clone(),
                    title: page.title,
                    body: page.body,
                },
            )
            .await?;
        pages
            .set_page_seo(actor, &id, &page.meta_title, &page.meta_description)
            .await?;
        pages.publish_page(actor, &id).await?;
        created += 1;
    }
    Ok((created, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_file_is_a_header_and_markdown() {
        let page = parse_page(
            "cgv".into(),
            "---\ntitle: Conditions : vente\nmeta_description: Les règles.\n---\n## Prix\n\nEn euros.\n",
        )
        .unwrap();
        assert_eq!(
            page,
            SeedPage {
                slug: "cgv".into(),
                title: "Conditions : vente".into(),
                meta_title: String::new(),
                meta_description: "Les règles.".into(),
                body: "## Prix\n\nEn euros.".into(),
            }
        );
        assert!(parse_page("x".into(), "no header").is_err());
        assert!(parse_page("x".into(), "---\nmeta_title: T\n---\nbody").is_err());
    }

    #[test]
    fn the_chatounette_pages_read() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/chatounette/pages");
        let pages = read_pages(&dir).unwrap();
        assert_eq!(pages.len(), 6);
        assert!(
            pages
                .iter()
                .all(|page| timada_pages::normalize_slug(&page.slug).is_ok())
        );
    }
}
