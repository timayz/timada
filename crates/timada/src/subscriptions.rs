//! Long-running subscriptions that keep every read model up to date.

use anyhow::Result;
use evento::sql::RwSqlite;
use evento::subscription::Subscription;
use sqlx::SqlitePool;

/// Handles to every running read-model subscription.
pub struct Subscriptions(Vec<Subscription>);

/// Start all read-model subscriptions.
///
/// `write_pool` must be the single-connection write pool from
/// [`crate::db::create_write_pool`], so read-model writes serialize through
/// the same connection as event-store commits.
pub async fn start(executor: &RwSqlite, write_pool: SqlitePool) -> Result<Subscriptions> {
    let subscriptions = vec![
        crate::read_model::provider_list::start(executor, write_pool.clone()).await?,
    ];

    tracing::info!("read-model subscriptions started");
    Ok(Subscriptions(subscriptions))
}

impl Subscriptions {
    /// Gracefully stop every subscription, awaiting in-flight events.
    pub async fn shutdown(self) -> Result<()> {
        for subscription in self.0 {
            subscription.shutdown().await?;
        }
        Ok(())
    }
}
