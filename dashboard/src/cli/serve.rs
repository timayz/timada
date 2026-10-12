use std::sync::Arc;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::db::{self, ReadPool, WritePool};
use crate::{app, auth, cluster};

pub async fn serve(config: Config, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or_else(|| config.server.host.clone());
    let port = port.unwrap_or(config.server.port);
    let url = config.server.url.clone();

    // Write pool first: it creates the file a read-only connection cannot.
    let write_pool = db::create_write_pool(&config.database.url).await?;
    let read_pool =
        db::create_read_pool(&config.database.url, config.database.max_connections).await?;
    let executor = cluster::build(
        config.cluster.as_ref(),
        config.server.region.as_deref(),
        read_pool.clone(),
        write_pool.clone(),
    )
    .await?;

    // Sign-in providers are resolved through discovery now, so a bad issuer
    // or network stops the server with the provider's name instead of
    // failing a sign-in later.
    let providers = auth::oidc::Providers::discover(&config.auth, &config.server.url).await?;
    let webauthn = auth::webauthn::build(&config.auth, &config.server.url)?;
    let mailer = auth::mail::build(&config.mail)?;

    // Read models catch up from their saved cursor before the router takes
    // requests, then follow the stream. A failed worker is logged and the
    // pages fall back to stale rows; the node keeps serving.
    let read_models = Arc::new(
        timada_identity::read_model_subscription(write_pool.clone())
            .start(&executor)
            .await
            .context("cannot start the identity read models")?,
    );
    let watched = read_models.clone();
    tokio::spawn(async move {
        let reason = watched.stopped().await;
        if reason.is_failure() {
            tracing::error!(%reason, "identity read models stopped; admin pages are stale");
        }
    });

    let router = app::router(
        executor,
        ReadPool(read_pool.clone()),
        WritePool(write_pool.clone()),
        providers,
        webauthn,
        mailer,
        config,
    );

    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!(addr = %listener.local_addr()?, "server listening");
    tracing::info!("open {url}");

    // Ctrl+C or SIGTERM starts graceful shutdown; in-flight requests get
    // `RouterService::shutdown_timeout` to finish.
    topcoat::serve(listener, router).await?;

    read_models.stop();
    read_models.stopped().await;
    read_pool.close().await;
    write_pool.close().await;
    Ok(())
}
