//! The connection to the store and the local copy of its catalog.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use evento::subscription::Subscription;
use evento::{Evento, Executor as _, RemoteClient, Sidecar};
use sqlx::SqlitePool;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::StoreConfig;
use crate::read_model::catalog_subscription;

/// How long the supervisor waits before restarting a failed subscription,
/// doubling up to [`MAX_RESTART_DELAY`].
const FIRST_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);

/// A connected store. Cheap to clone; every clone shares the connection, the
/// local database and the catalog subscription.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    executor: Evento,
    pool: SqlitePool,
    prefix: String,
    media_url: Option<String>,
    request_timeout: Duration,
    subscription: Mutex<SubscriptionState>,
    stop: watch::Sender<bool>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

/// What the status endpoint reports.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StoreStatus {
    /// Whether the store answered a request just now.
    pub connected: bool,
    pub subscription: SubscriptionState,
    /// Published products in the local copy.
    pub products: i64,
    /// Unix seconds of the last catalog change seen.
    pub last_event_at: Option<i64>,
}

/// Where the catalog subscription stands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubscriptionState {
    Running,
    /// A pass kept failing (the store was unreachable for a while); the
    /// subscription is restarted from its local cursor.
    Restarting {
        error: String,
    },
    Stopped {
        reason: String,
    },
}

impl Store {
    /// Connects to the store (failing at once when it is unreachable or
    /// refuses the key), prepares `pool` (evento's tables and the library's
    /// own) and starts following the catalog into it.
    pub async fn connect(config: StoreConfig, pool: SqlitePool) -> anyhow::Result<Store> {
        let mut builder =
            RemoteClient::builder(config.addr).request_timeout(config.request_timeout);
        builder = builder.token(config.token.clone());
        if let Some(tls) = config.tls.clone() {
            builder = builder.tls(tls.connector, tls.server_name);
        }
        let client = builder
            .connect()
            .await
            .with_context(|| format!("cannot connect to the store at {}", config.addr))?;
        // Events on the store, cursors and snapshots here. The client carries
        // the store's routing key, which prefixes the cursor key.
        let local: evento::Sqlite = pool.clone().into();
        let executor = Evento::new(Sidecar::from((client, local)));

        crate::migrate(&pool).await?;

        let subscription = catalog_subscription(pool.clone())
            .start(&executor)
            .await
            .context("cannot start following the catalog")?;
        let (stop, stopped) = watch::channel(false);
        let inner = Arc::new(Inner {
            executor,
            pool,
            prefix: config.prefix.into_owned(),
            media_url: config.media_url,
            request_timeout: config.request_timeout,
            subscription: Mutex::new(SubscriptionState::Running),
            stop,
            supervisor: Mutex::new(None),
        });
        let supervisor = tokio::spawn(supervise(inner.clone(), subscription, stopped));
        *inner.supervisor.lock().unwrap() = Some(supervisor);
        tracing::info!(addr = %config.addr, "store connected");
        Ok(Store { inner })
    }

    /// The store's executor: events are read from and written to the store,
    /// subscription cursors and snapshots stay local.
    pub fn executor(&self) -> &Evento {
        &self.inner.executor
    }

    /// The local database: the catalog copy, cursors and snapshots.
    pub fn pool(&self) -> &SqlitePool {
        &self.inner.pool
    }

    /// The URL prefix of the library's endpoints.
    pub fn prefix(&self) -> &str {
        &self.inner.prefix
    }

    /// Where this store's pictures are loaded from, when the host said so.
    pub fn media_url(&self) -> Option<&str> {
        self.inner.media_url.as_deref()
    }

    /// Asks the store one cheap question and counts the local copy.
    pub async fn status(&self) -> StoreStatus {
        let probe = self.inner.executor.latest_timestamp(None, None);
        let connected = tokio::time::timeout(self.inner.request_timeout, probe)
            .await
            .map(|answer| answer.is_ok())
            .unwrap_or(false);
        let products: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM storefront_products WHERE status = 'published'",
        )
        .fetch_one(&self.inner.pool)
        .await
        .unwrap_or(0);
        let last_event_at: Option<i64> =
            sqlx::query_scalar("SELECT max(updated_at) FROM storefront_products")
                .fetch_one(&self.inner.pool)
                .await
                .unwrap_or(None);
        StoreStatus {
            connected,
            subscription: self.inner.subscription.lock().unwrap().clone(),
            products,
            last_event_at,
        }
    }

    /// Stops following the catalog. The local database is left as it is and
    /// resumes from its cursor on the next connection.
    pub async fn shutdown(self) {
        let _ = self.inner.stop.send(true);
        let supervisor = self.inner.supervisor.lock().unwrap().take();
        if let Some(supervisor) = supervisor {
            let _ = supervisor.await;
        }
    }
}

/// Keeps the catalog subscription alive: a pass that kept failing (the store
/// was away longer than the subscription's own retries) ends the worker, so
/// it is started again from the local cursor after a growing delay. Without
/// this the local copy would silently freeze.
async fn supervise(inner: Arc<Inner>, mut handle: Subscription, mut stop: watch::Receiver<bool>) {
    let set = |state: SubscriptionState| {
        *inner.subscription.lock().unwrap() = state;
    };
    let mut delay = FIRST_RESTART_DELAY;
    loop {
        let reason = tokio::select! {
            _ = stop.changed() => {
                handle.shutdown().await.ok();
                set(SubscriptionState::Stopped { reason: "shutdown".to_owned() });
                return;
            }
            reason = handle.stopped() => reason,
        };
        if !reason.is_failure() {
            set(SubscriptionState::Stopped {
                reason: format!("{reason:?}"),
            });
            return;
        }
        let error = reason
            .error()
            .map(|err| format!("{err:#}"))
            .unwrap_or_else(|| format!("{reason:?}"));
        tracing::warn!(%error, "the catalog subscription stopped, restarting");
        set(SubscriptionState::Restarting { error });

        handle = loop {
            tokio::select! {
                _ = stop.changed() => {
                    set(SubscriptionState::Stopped { reason: "shutdown".to_owned() });
                    return;
                }
                _ = tokio::time::sleep(delay) => {}
            }
            delay = (delay * 2).min(MAX_RESTART_DELAY);
            match catalog_subscription(inner.pool.clone())
                .start(&inner.executor)
                .await
            {
                Ok(handle) => break handle,
                Err(err) => {
                    let error = format!("{err:#}");
                    tracing::warn!(%error, "the catalog subscription cannot restart yet");
                    set(SubscriptionState::Restarting { error });
                }
            }
        };
        delay = FIRST_RESTART_DELAY;
        set(SubscriptionState::Running);
        tracing::info!("the catalog subscription is running again");
    }
}
