use anyhow::{Context, Result};

use crate::config::Config;
use crate::{app, cluster, db};

pub async fn serve(config: Config, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or_else(|| config.server.host.clone());
    let port = port.unwrap_or(config.server.port);

    let pool = db::create_pool(
        &config.database.url,
        config.database.max_connections,
        db::SERVE_BUSY_TIMEOUT,
    )
    .await?;
    let executor = cluster::build(config.cluster.as_ref(), pool.clone()).await?;

    // Subscriptions start here, before the router takes requests. Keep each
    // handle and `shutdown()` them after `topcoat::serve` returns.

    let router = app::router(executor, pool.clone(), config);

    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!(addr = %listener.local_addr()?, "server listening");

    // Ctrl+C or SIGTERM starts graceful shutdown; in-flight requests get
    // `RouterService::shutdown_timeout` to finish.
    topcoat::serve(listener, router).await?;

    pool.close().await;
    Ok(())
}
