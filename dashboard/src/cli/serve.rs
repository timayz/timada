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
    let media = Arc::new(crate::media::Media::new(
        &config.media,
        config.cluster.as_ref(),
    )?);
    let tenants = Tenants::new(
        cluster.clone(),
        &config.tenants,
        config.server.region.clone(),
        executor.clone(),
        write_pool.clone(),
        tenant_subscriptions(media.clone()),
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
        // Not a read model, but the same lifecycle: every node opens a tenant
        // as soon as its creation arrives.
        start_read_model(
            "new tenants",
            crate::tenants::creation_subscription(tenants.clone()),
            &executor,
        )
        .await?,
    ];

    // A restarted node brings its tenant copies up to date in the background
    // instead of waiting for something to open each one.
    let catch_up = tenants.start_catch_up(CatchUp::default());

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
/// fed into the shared database (the catalog's product list), and this
/// node's copies of the product pictures. Every tenant-scoped bounded
/// context registers its subscription here.
fn tenant_subscriptions(media: Arc<crate::media::Media>) -> Vec<TenantSubscription> {
    vec![
        Arc::new(timada_catalog::read_model_subscription::<Executor>),
        Arc::new(move |_, tenant_id| crate::media::subscription(media.clone(), tenant_id)),
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
