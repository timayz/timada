use std::sync::Arc;

use anyhow::{Context, Result};
use evento::subscription::Subscription;

use crate::app::{self, Services};
use crate::auth;
use crate::cluster::{Cluster, Executor};
use crate::config::Config;
use crate::db::{self, ReadPool, WritePool};
use crate::tenants::{TenantSubscription, Tenants};

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
    let tenants = Tenants::new(
        cluster.clone(),
        &config.tenants,
        config.server.region.clone(),
        executor.clone(),
        write_pool.clone(),
        tenant_subscriptions(),
    );
    let sweeper = tenants.start_sweeper();
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
    ];

    let router = app::router(Services {
        executor,
        read_pool: ReadPool(read_pool.clone()),
        write_pool: WritePool(write_pool.clone()),
        providers,
        webauthn,
        mailer,
        tenants: tenants.clone(),
        authorizer,
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

    sweeper.abort();
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

/// The read models fed from each tenant's own stream into the shared
/// database. None yet: the first tenant-scoped bounded context registers its
/// subscription here.
fn tenant_subscriptions() -> Vec<TenantSubscription> {
    Vec::new()
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
