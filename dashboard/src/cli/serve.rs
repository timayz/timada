use std::sync::Arc;

use anyhow::{Context, Result};
use evento::subscription::Subscription;

use crate::app::{self, Services};
use crate::auth;
use crate::cluster::{Cluster, Executor};
use crate::config::Config;
use crate::db::{self, ReadPool, WritePool};
use crate::store_api::{self, StoreConnections};
use crate::tenants::{CatchUp, TenantSubscription, Tenants};

pub async fn serve(config: Config, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or_else(|| config.server.host.clone());
    let port = port.unwrap_or(config.server.port);
    let url = config.server.url.clone();

    // Write pool first: it creates the file a read-only connection cannot.
    let write_pool = db::create_write_pool(&config.database.url).await?;
    let read_pool =
        db::create_read_pool(&config.database.url, config.database.max_connections).await?;
    let cluster = Arc::new(Cluster::build(config.cluster.as_ref()).await?);
    let executor = cluster
        .open_platform(
            config.server.region.as_deref(),
            read_pool.clone(),
            write_pool.clone(),
        )
        .await?;
    let peers = crate::peers::Peers::from_cluster(config.cluster.as_ref())?.map(Arc::new);
    let media = Arc::new(crate::media::Media::new(&config.media, peers.clone()));
    let keys_sealer = crate::seal::keys_sealer(&config);
    if keys_sealer.is_none() {
        tracing::warn!(
            "secrets.key is not set: no shopper can create an account and no contact message can be received"
        );
    }
    let keys = Arc::new(crate::key_vault::KeyVault::new(
        &config.data_keys,
        keys_sealer,
        peers,
    ));
    let gateway = Arc::new(crate::payments::StripeGateway::new(
        config
            .payments
            .as_ref()
            .and_then(|payments| payments.stripe_api_url.clone()),
    ));
    let payments = Arc::new(crate::payments::Payments::new(
        crate::seal::payments_sealer(&config),
        gateway,
        executor.clone(),
    ));
    // The shops' own mail servers, shared by the pages and the mails sent
    // in the background so their connections are too.
    let shop_mail = crate::shop_mail::ShopMail::new(
        crate::seal::mail_sealer(&config),
        config.mail.allow_private_hosts,
    );
    let aliexpress = crate::aliexpress::Aliexpress::new(
        crate::seal::aliexpress_sealer(&config),
        &config.aliexpress,
        config.mail.allow_private_hosts,
        executor.clone(),
    )?;
    // The order confirmations and the notices of contact messages are
    // mailed from the tenants' subscriptions, outside any request: they get
    // their own transport.
    let background_mail = {
        let mailer = auth::mail::build(&config.mail)?;
        let mut app = topcoat::context::AppContext::new();
        app.insert(mailer.transport);
        app.insert(mailer.sender);
        app.insert(shop_mail.clone());
        // Where the shops' logos are loaded from.
        app.insert(media.clone());
        Arc::new(app)
    };
    let tenants = Tenants::new(
        cluster.clone(),
        &config.tenants,
        config.server.region.clone(),
        executor.clone(),
        write_pool.clone(),
        tenant_subscriptions(
            media.clone(),
            keys.clone(),
            payments.clone(),
            aliexpress.clone(),
            executor.clone(),
            Arc::new(crate::vies::ViesClient::new(
                config.invoicing.vies_url.clone(),
            )),
            background_mail.clone(),
            read_pool.clone(),
            url.clone(),
        ),
    );
    let connections = StoreConnections::new();
    let store_api = store_api::serve(
        config.store_api.as_ref(),
        executor.clone(),
        tenants.clone(),
        connections.clone(),
    )
    .await?;
    let authorizer = crate::authz::Authorizer::new().await?;

    // Sign-in providers are resolved through discovery now, so a bad issuer
    // or network stops the server with the provider's name instead of
    // failing a sign-in later.
    let providers = auth::oidc::Providers::discover(&config.auth, &config.server.url).await?;
    let webauthn = auth::webauthn::build(&config.auth, &config.server.url)?;
    let mailer = auth::mail::build(&config.mail)?;

    // Read models catch up from their saved cursor before the router takes
    // requests, then follow the stream. A failed worker is logged and the
    // pages fall back to stale rows; the node keeps serving.
    let read_models = vec![
        start_read_model(
            "identity",
            timada_identity::read_model_subscription(write_pool.clone()),
            &executor,
        )
        .await?,
        start_read_model(
            "tenancy",
            timada_tenancy::read_model_subscription(write_pool.clone()),
            &executor,
        )
        .await?,
        start_read_model(
            "members history",
            timada_tenancy::member_audit_subscription(write_pool.clone()),
            &executor,
        )
        .await?,
        // Not a read model, but the same lifecycle: every node opens a tenant
        // as soon as its creation arrives.
        start_read_model(
            "new tenants",
            crate::tenants::creation_subscription(tenants.clone()),
            &executor,
        )
        .await?,
        // Nor this one: it copies how each tenant's shoppers pay into the
        // tenant's store, for the storefronts' payment form.
        start_read_model(
            "payment setup",
            crate::payments::setup_subscription(payments.clone(), tenants.clone()),
            &executor,
        )
        .await?,
        // Nor this: it fetches the shops' mail logos this node lacks.
        start_read_model(
            "mail logos",
            crate::media::logo_subscription(media.clone()),
            &executor,
        )
        .await?,
    ];

    // A restarted node brings its tenant copies up to date in the background
    // instead of waiting for something to open each one.
    let catch_up = tenants.start_catch_up(CatchUp::default());
    // The operator's alert for a node that stays away: a tenant nearing the
    // writes it may keep for it, or a node given up on.
    let backlog_watch = tenants.watch_backlog(std::time::Duration::from_secs(60));
    // A few days after an order left, its buyer is asked for a review.
    let review_requests = crate::review_requests::ReviewRequests::new(
        read_pool.clone(),
        executor.clone(),
        tenants.clone(),
        background_mail,
    )
    .start(std::time::Duration::from_secs(
        config.reviews.request_check_secs.max(1),
    ));

    let router = app::router(Services {
        executor,
        read_pool: ReadPool(read_pool.clone()),
        write_pool: WritePool(write_pool.clone()),
        providers,
        webauthn,
        mailer,
        tenants: tenants.clone(),
        authorizer,
        media,
        keys,
        payments,
        shop_mail,
        aliexpress,
        store: connections,
        config,
    });

    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!(addr = %listener.local_addr()?, "server listening");
    tracing::info!("open {url}");

    // Ctrl+C or SIGTERM starts graceful shutdown; in-flight requests get
    // `RouterService::shutdown_timeout` to finish.
    topcoat::serve(listener, router).await?;

    // Store connections go first: their leases hold tenants open.
    if let Some(store_api) = store_api {
        store_api.shutdown().await;
    }
    if let Some(catch_up) = catch_up {
        catch_up.abort();
    }
    backlog_watch.abort();
    review_requests.abort();
    tenants.shutdown().await;
    for subscription in &read_models {
        subscription.stop();
    }
    for subscription in &read_models {
        subscription.stopped().await;
    }
    read_pool.close().await;
    write_pool.close().await;
    Ok(())
}

/// What follows each tenant's own stream while it is open: the read models
/// fed into the shared database (the catalog's product list, the orders and
/// carts, the customers, the invoices, the reviews, the support messages), this node's copies of the product
/// pictures, review photos and personal-data keys, the answers the dashboard writes back
/// (payments, invoices), the order confirmations it mails and the notices
/// of contact messages it mails the members. Every tenant-scoped bounded
/// context registers its subscription here.
#[allow(clippy::too_many_arguments)]
fn tenant_subscriptions(
    media: Arc<crate::media::Media>,
    keys: Arc<crate::key_vault::KeyVault>,
    payments: Arc<crate::payments::Payments>,
    aliexpress: Arc<crate::aliexpress::Aliexpress>,
    platform: Executor,
    vies: Arc<dyn crate::vies::Vies>,
    background_mail: Arc<topcoat::context::AppContext>,
    read_pool: sqlx::SqlitePool,
    base_url: String,
) -> Vec<TenantSubscription> {
    vec![
        Arc::new(|pool, tenant_id, _: &Executor| {
            timada_catalog::read_model_subscription(pool, tenant_id)
        }),
        Arc::new(|pool, tenant_id, _: &Executor| {
            timada_orders::read_model_subscription(pool, tenant_id)
        }),
        Arc::new({
            let keys = keys.clone();
            move |pool, tenant_id, _: &Executor| {
                timada_shoppers::read_model_subscription(
                    pool,
                    tenant_id,
                    keys.for_tenant(tenant_id),
                )
            }
        }),
        Arc::new(|pool, tenant_id, _: &Executor| {
            timada_discounts::read_model_subscription(pool, tenant_id)
        }),
        Arc::new(|pool, tenant_id, _: &Executor| {
            timada_reviews::read_model_subscription(pool, tenant_id)
        }),
        Arc::new({
            let keys = keys.clone();
            move |pool, tenant_id, _: &Executor| {
                timada_support::read_model_subscription(pool, tenant_id, keys.for_tenant(tenant_id))
            }
        }),
        Arc::new(|pool, tenant_id, _: &Executor| {
            timada_invoicing::read_model_subscription(pool, tenant_id)
        }),
        Arc::new(|pool, tenant_id, _: &Executor| {
            timada_aliexpress::read_model_subscription(pool, tenant_id)
        }),
        Arc::new({
            let media = media.clone();
            move |_, tenant_id, executor: &Executor| {
                crate::aliexpress::subscription::subscription(
                    aliexpress.clone(),
                    media.clone(),
                    tenant_id,
                    executor.clone(),
                )
            }
        }),
        Arc::new(move |_, tenant_id, _: &Executor| {
            crate::media::subscription(media.clone(), tenant_id)
        }),
        Arc::new({
            let keys = keys.clone();
            move |_, tenant_id, _: &Executor| {
                crate::shopper_keys::subscription(keys.clone(), tenant_id)
            }
        }),
        Arc::new({
            let keys = keys.clone();
            move |_, tenant_id, _: &Executor| {
                crate::message_keys::subscription(keys.clone(), tenant_id)
            }
        }),
        Arc::new(move |_, tenant_id, executor: &Executor| {
            crate::payments::subscription(payments.clone(), tenant_id, executor.clone())
        }),
        Arc::new(|_, _, executor: &Executor| {
            timada_orders::discount_usage_subscription(executor.clone())
        }),
        Arc::new(|_, _, executor: &Executor| {
            timada_orders::stock_usage_subscription(executor.clone())
        }),
        Arc::new({
            let platform = platform.clone();
            let background_mail = background_mail.clone();
            move |_, tenant_id, executor: &Executor| {
                crate::confirmations::subscription(
                    platform.clone(),
                    background_mail.clone(),
                    tenant_id,
                    executor.clone(),
                )
            }
        }),
        Arc::new({
            let platform = platform.clone();
            move |_, tenant_id, executor: &Executor| {
                crate::message_notifications::subscription(
                    platform.clone(),
                    read_pool.clone(),
                    background_mail.clone(),
                    base_url.clone(),
                    keys.for_tenant(tenant_id),
                    tenant_id,
                    executor.clone(),
                )
            }
        }),
        Arc::new(move |_, tenant_id, executor: &Executor| {
            crate::invoicing::subscription(
                platform.clone(),
                vies.clone(),
                tenant_id,
                executor.clone(),
            )
        }),
    ]
}

async fn start_read_model(
    name: &'static str,
    builder: evento::subscription::SubscriptionBuilder<Executor>,
    executor: &Executor,
) -> Result<Arc<Subscription>> {
    let subscription = Arc::new(
        builder
            .start(executor)
            .await
            .with_context(|| format!("cannot start the {name} read models"))?,
    );
    let watched = subscription.clone();
    tokio::spawn(async move {
        let reason = watched.stopped().await;
        if reason.is_failure() {
            tracing::error!(%reason, "{name} read models stopped; their pages are stale");
        }
    });
    Ok(subscription)
}
