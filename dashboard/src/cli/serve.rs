use std::sync::Arc;

use anyhow::{Context, Result};
use evento::subscription::{Subscription, SubscriptionBuilder};

use crate::app::{self, Services};
use crate::auth;
use crate::cluster::{Cluster, Executor, Files, StorePools};
use crate::config::Config;
use crate::db::{self, ReadPool, WritePool};
use crate::store_api::{self, StoreConnections};

pub async fn serve(config: Config, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or_else(|| config.server.host.clone());
    let port = port.unwrap_or(config.server.port);
    let url = config.server.url.clone();
    let files = Files {
        store_url: config.database.store.clone(),
        read_models_url: config.database.read_models.clone(),
    };

    // The store first: its node may have to copy it from a peer, which
    // recreates the read models, so their pools open afterwards. Write pool
    // before read pool: it creates the file a read-only connection cannot.
    let store_write =
        db::create_write_pool(&config.database.store, db::Durability::Durable).await?;
    let store_cursors =
        db::create_write_pool(&config.database.store, db::Durability::Relaxed).await?;
    let store_read =
        db::create_read_pool(&config.database.store, config.database.max_connections).await?;
    let (cluster, executor) = Cluster::open(
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
    // mailed from the subscriptions, outside any request: they get their
    // own transport.
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
    let connections = StoreConnections::new();
    let store_api = store_api::serve(
        config.store_api.as_ref(),
        executor.clone(),
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
    let mut read_models = Vec::new();
    for (name, builder) in subscriptions(
        write_pool.clone(),
        read_pool.clone(),
        executor.clone(),
        media.clone(),
        keys.clone(),
        payments.clone(),
        aliexpress.clone(),
        Arc::new(crate::vies::ViesClient::new(
            config.invoicing.vies_url.clone(),
        )),
        background_mail.clone(),
        url.clone(),
    ) {
        read_models.push(start_read_model(name, builder, &executor).await?);
    }

    // The operator's alert for a node that stays away: this node nearing the
    // writes it may keep for it, or a node given up on.
    let cluster_watch = cluster.watch(std::time::Duration::from_secs(60));
    // A few days after an order left, its buyer is asked for a review.
    let review_requests = crate::review_requests::ReviewRequests::new(
        read_pool.clone(),
        executor.clone(),
        background_mail.clone(),
    )
    .start(std::time::Duration::from_secs(
        config.reviews.request_check_secs.max(1),
    ));

    // A campaign a merchant sent goes out a batch at a time.
    let newsletter_sends = crate::newsletter_sends::NewsletterSends::new(
        read_pool.clone(),
        executor.clone(),
        background_mail.clone(),
        keys.clone(),
        config.newsletter.mails_per_check,
    )
    .start(std::time::Duration::from_secs(
        config.newsletter.send_check_secs.max(1),
    ));

    // The products imported from AliExpress are read again on a timer.
    let aliexpress_sync = crate::aliexpress::sync::AliexpressSync::new(
        read_pool.clone(),
        executor.clone(),
        aliexpress.clone(),
    )
    .start(std::time::Duration::from_secs(
        config.aliexpress.sync_check_secs.max(1),
    ));

    let router = app::router(Services {
        executor,
        read_pool: ReadPool(read_pool.clone()),
        write_pool: WritePool(write_pool.clone()),
        providers,
        webauthn,
        mailer,
        cluster: cluster.clone(),
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

    if let Some(store_api) = store_api {
        store_api.shutdown().await;
    }
    cluster_watch.abort();
    review_requests.abort();
    newsletter_sends.abort();
    aliexpress_sync.abort();
    for subscription in &read_models {
        subscription.stop();
    }
    for subscription in &read_models {
        subscription.stopped().await;
    }
    cluster.shutdown().await;
    read_pool.close().await;
    write_pool.close().await;
    store_read.close().await;
    store_cursors.close().await;
    store_write.close().await;
    Ok(())
}

/// Every subscription this node runs on the store, each following every
/// tenant's streams: the platform's read models (the users, the tenant
/// catalog, the members' history), the tenants' read models fed into the
/// read-model database (the catalogs, the orders and carts, the customers,
/// the invoices, the reviews, the support messages), this node's copies of
/// the product pictures, review photos and personal-data keys, the answers
/// the dashboard writes back (payments, invoices), the order confirmations it
/// mails and the notices of contact messages it mails the members. Every
/// bounded context registers its subscription here.
#[allow(clippy::too_many_arguments)]
fn subscriptions(
    write_pool: sqlx::SqlitePool,
    read_pool: sqlx::SqlitePool,
    store: Executor,
    media: Arc<crate::media::Media>,
    keys: Arc<crate::key_vault::KeyVault>,
    payments: Arc<crate::payments::Payments>,
    aliexpress: Arc<crate::aliexpress::Aliexpress>,
    vies: Arc<dyn crate::vies::Vies>,
    background_mail: Arc<topcoat::context::AppContext>,
    base_url: String,
) -> Vec<(&'static str, SubscriptionBuilder<Executor>)> {
    let ring = keys.ring();
    vec![
        (
            "identity",
            timada_identity::read_model_subscription(write_pool.clone()),
        ),
        (
            "tenancy",
            timada_tenancy::read_model_subscription(write_pool.clone()),
        ),
        (
            "members history",
            timada_tenancy::member_audit_subscription(write_pool.clone()),
        ),
        (
            "AliExpress access",
            timada_tenancy::aliexpress_accounts_subscription(write_pool.clone()),
        ),
        // Not a read model, but the same lifecycle: it copies how each
        // tenant's shoppers pay into the tenant's streams, for the
        // storefronts' payment form.
        (
            "payment setup",
            crate::payments::setup_subscription(payments.clone(), store.clone()),
        ),
        // Nor this: it fetches the shops' mail logos this node lacks.
        ("mail logos", crate::media::logo_subscription(media.clone())),
        (
            "catalog",
            timada_catalog::read_model_subscription(write_pool.clone()),
        ),
        (
            "orders",
            timada_orders::read_model_subscription(write_pool.clone()),
        ),
        (
            "shoppers",
            timada_shoppers::read_model_subscription(write_pool.clone(), ring.clone()),
        ),
        (
            "discounts",
            timada_discounts::read_model_subscription(write_pool.clone()),
        ),
        (
            "reviews",
            timada_reviews::read_model_subscription(write_pool.clone()),
        ),
        (
            "support",
            timada_support::read_model_subscription(write_pool.clone(), ring.clone()),
        ),
        (
            "newsletter",
            timada_newsletter::read_model_subscription(write_pool.clone(), ring.clone()),
        ),
        (
            "automations",
            timada_automations::read_model_subscription(write_pool.clone()),
        ),
        (
            "invoicing",
            timada_invoicing::read_model_subscription(write_pool.clone()),
        ),
        (
            "AliExpress",
            timada_aliexpress::read_model_subscription(write_pool.clone()),
        ),
        (
            "AliExpress orders",
            crate::aliexpress::forward::subscription(
                aliexpress.clone(),
                write_pool.clone(),
                store.clone(),
            ),
        ),
        (
            "AliExpress products",
            crate::aliexpress::subscription::subscription(
                aliexpress.clone(),
                media.clone(),
                store.clone(),
            ),
        ),
        ("pictures", crate::media::subscription(media.clone())),
        (
            "shopper keys",
            crate::shopper_keys::subscription(keys.clone()),
        ),
        (
            "message keys",
            crate::message_keys::subscription(keys.clone()),
        ),
        (
            "newsletter keys",
            crate::newsletter_keys::subscription(keys.clone()),
        ),
        (
            "payments",
            crate::payments::subscription(payments.clone(), store.clone()),
        ),
        (
            "discount usage",
            timada_orders::discount_usage_subscription(store.clone()),
        ),
        (
            "stock usage",
            timada_orders::stock_usage_subscription(store.clone()),
        ),
        (
            "newsletter opt-ins",
            crate::newsletter::subscription(
                store.clone(),
                background_mail.clone(),
                keys.clone(),
                store.clone(),
            ),
        ),
        (
            "order confirmations",
            crate::confirmations::subscription(
                store.clone(),
                background_mail.clone(),
                store.clone(),
            ),
        ),
        (
            "message notices",
            crate::message_notifications::subscription(
                store.clone(),
                read_pool,
                background_mail,
                base_url,
                ring,
                store.clone(),
            ),
        ),
        (
            "invoices",
            crate::invoicing::subscription(store.clone(), vies, store),
        ),
    ]
}

async fn start_read_model(
    name: &'static str,
    builder: SubscriptionBuilder<Executor>,
    executor: &Executor,
) -> Result<Arc<Subscription>> {
    let subscription = Arc::new(
        builder
            .start(executor)
            .await
            .with_context(|| format!("cannot start the {name} subscription"))?,
    );
    let watched = subscription.clone();
    tokio::spawn(async move {
        let reason = watched.stopped().await;
        if reason.is_failure() {
            tracing::error!(%reason, "the {name} subscription stopped; what it feeds is stale");
        }
    });
    Ok(subscription)
}
