//! The connection to the store and the local copy of its catalog.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use evento::subscription::Subscription;
use evento::{Evento, Executor as _, RemoteClient, Sidecar};
use timada_core::db::Db;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::StoreConfig;
use crate::db::StoreDb;
use crate::error::StoreUnavailable;
use crate::money::MoneyFormat;
use crate::read_model::catalog_subscription;

/// How long the supervisor waits before connecting or restarting the
/// subscription again, doubling up to [`MAX_RESTART_DELAY`].
const FIRST_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);

/// A connected store. Cheap to clone; every clone shares the connection, the
/// local database and the catalog subscription.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    config: StoreConfig,
    /// The store API, once reached: events are read from and written to it,
    /// cursors and snapshots stay local. Empty while the store has not
    /// answered yet (it was away when the shop started).
    executor: OnceLock<Evento>,
    db: StoreDb,
    /// The dashboard's HTTP side, when the host gave its address.
    api: Option<crate::reviews::Api>,
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
    /// How far the copy trails the store, in seconds: the age of the last
    /// event worked through against the newest in the store, as of the last
    /// time the subscription recorded its cursor. A copy catching up (a new
    /// replica, one that was away) reads high and falls; a balancer can keep
    /// shoppers off it until it reads low. Unknown until the subscription
    /// first ran.
    pub lag_seconds: Option<i64>,
}

/// Where the catalog subscription stands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubscriptionState {
    /// The store has not answered since the shop started: it is reached
    /// again after a growing delay, and the pages serve the local copy
    /// meanwhile.
    Connecting {
        error: String,
    },
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
    /// Prepares the local database (evento's tables and the library's own),
    /// connects to the store and starts following the catalog into it. `db`
    /// is [`StoreDb::open`]'s, or a store a test built in memory.
    ///
    /// A store that refuses the key fails at once: nothing would ever work.
    /// A store that cannot be reached does not: the shop opens on the
    /// catalog it has (empty, on a first start), the store is reached again
    /// after a growing delay (1 s to 30 s), and until then every call that
    /// needs it fails with [`StoreUnavailable`](crate::StoreUnavailable);
    /// [`status`](Self::status) says where it stands.
    pub async fn connect(config: StoreConfig, db: impl Into<StoreDb>) -> anyhow::Result<Store> {
        let db = db.into();
        crate::migrate(db.db()).await?;
        let api = match &config.api_url {
            Some(url) => Some(crate::reviews::Api::new(
                url,
                &config.token,
                config.request_timeout,
            )?),
            None => None,
        };
        let (stop, stopped) = watch::channel(false);
        let inner = Arc::new(Inner {
            config,
            executor: OnceLock::new(),
            db,
            api,
            subscription: Mutex::new(SubscriptionState::Connecting {
                error: String::new(),
            }),
            stop,
            supervisor: Mutex::new(None),
        });

        let subscription = match connect_client(&inner.config).await {
            Ok(client) => {
                inner.set_executor(client);
                tracing::info!(addr = %inner.config.addr, "store connected");
                Some(inner.start_subscription().await?)
            }
            Err(err) if err.downcast_ref::<evento::remote::Rejection>().is_some() => {
                return Err(err.context(format!(
                    "cannot connect to the store at {}",
                    inner.config.addr
                )));
            }
            Err(err) => {
                let error = format!("{err:#}");
                tracing::warn!(
                    addr = %inner.config.addr,
                    %error,
                    "the store cannot be reached: serving the local copy until it answers"
                );
                inner.set_state(SubscriptionState::Connecting { error });
                None
            }
        };
        let supervisor = tokio::spawn(supervise(inner.clone(), subscription, stopped));
        *inner.supervisor.lock().unwrap() = Some(supervisor);
        Ok(Store { inner })
    }

    /// The store's executor: events are read from and written to the store,
    /// subscription cursors and snapshots stay local. Fails while the store
    /// has not answered since the shop started.
    pub fn executor(&self) -> Result<&Evento, StoreUnavailable> {
        self.inner
            .executor
            .get()
            .ok_or_else(|| StoreUnavailable::new("the store API has not answered yet"))
    }

    /// The local database, for reading: the catalog copy, cursors and
    /// snapshots. Only the library writes to it.
    pub fn db(&self) -> &Db {
        self.inner.db.db()
    }

    /// The URL prefix of the library's endpoints.
    pub fn prefix(&self) -> &str {
        &self.inner.config.prefix
    }

    /// Where this store's pictures are loaded from, when the host said so.
    pub fn media_url(&self) -> Option<&str> {
        self.inner.config.media_url.as_deref()
    }

    /// How every amount a person reads is written
    /// ([`StoreConfig::money`](crate::StoreConfig::money)). The views the
    /// library builds carry it; this is for code that formats on its own.
    pub fn money(&self) -> &MoneyFormat {
        &self.inner.config.money
    }

    /// Whether reviews can be handed in: the host gave the dashboard's
    /// address ([`StoreConfig::api_url`](crate::StoreConfig::api_url)).
    /// [`takes_messages`](Self::takes_messages) says the same of the
    /// contact form.
    pub fn takes_reviews(&self) -> bool {
        self.inner.api.is_some()
    }

    /// The dashboard's HTTP side, when the host gave its address.
    pub(crate) fn api(&self) -> Option<&crate::reviews::Api> {
        self.inner.api.as_ref()
    }

    /// Asks the store one cheap question and counts the local copy.
    pub async fn status(&self) -> StoreStatus {
        let connected = match self.inner.executor.get() {
            Some(executor) => {
                let probe = executor.latest_timestamp(None, None, evento::TenantFilter::All);
                tokio::time::timeout(self.inner.config.request_timeout, probe)
                    .await
                    .map(|answer| answer.is_ok())
                    .unwrap_or(false)
            }
            None => false,
        };
        let db = self.inner.db.db();
        let products: i64 = db
            .fetch_scalar(
                "SELECT count(*) FROM storefront_products WHERE status = 'published'",
                (),
            )
            .await
            .unwrap_or(0);
        let last_event_at: Option<i64> = db
            .fetch_scalar("SELECT max(updated_at) FROM storefront_products", ())
            .await
            .unwrap_or(None);
        // The subscription writes its lag next to its cursor each time it
        // records it.
        let lag_seconds: Option<i64> = db
            .fetch_optional::<(i64,)>(
                "SELECT lag FROM subscriber WHERE key = ?1",
                (crate::read_model::CATALOG_SUBSCRIPTION,),
            )
            .await
            .ok()
            .flatten()
            .map(|(lag,)| lag);
        StoreStatus {
            connected,
            subscription: self.inner.subscription.lock().unwrap().clone(),
            products,
            last_event_at,
            lag_seconds,
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

impl Inner {
    fn set_state(&self, state: SubscriptionState) {
        *self.subscription.lock().unwrap() = state;
    }

    /// Keeps the connection: events on the store, cursors and snapshots
    /// here. The client carries the store's routing key (told by the
    /// server's Hello); the wrapper keeps it, so it prefixes the cursor key
    /// and keys new aggregates.
    fn set_executor(&self, client: RemoteClient) {
        let executor = Evento::new(Sidecar::from((client, self.db.store().clone())));
        // Set once the store first answers; a later call would be a bug of
        // the supervisor, not a reason to drop the connection in use.
        let _ = self.executor.set(executor);
    }

    async fn start_subscription(&self) -> anyhow::Result<Subscription> {
        let executor = self
            .executor
            .get()
            .context("the store API has not answered yet")?;
        let subscription = catalog_subscription(self.db.db().clone())
            .start(executor)
            .await
            .context("cannot start following the catalog")?;
        self.set_state(SubscriptionState::Running);
        Ok(subscription)
    }
}

/// Connects to the store API and performs its handshake; the client then
/// reconnects on its own when the connection drops.
async fn connect_client(config: &StoreConfig) -> anyhow::Result<RemoteClient> {
    let mut builder =
        RemoteClient::builder(config.addr.clone()).request_timeout(config.request_timeout);
    builder = builder.token(config.token.clone());
    if let Some(tls) = config.tls.clone() {
        builder = builder.tls(tls.connector, tls.server_name);
    }
    builder.connect().await
}

/// Keeps the catalog subscription alive. A store that was away when the shop
/// started is reached again after a growing delay, and the subscription
/// started once it answers. A pass that kept failing (the store was away
/// longer than the subscription's own retries) ends the worker, so it is
/// started again from the local cursor after a growing delay. Without this
/// the local copy would silently freeze.
async fn supervise(
    inner: Arc<Inner>,
    mut handle: Option<Subscription>,
    mut stop: watch::Receiver<bool>,
) {
    let stopped = || {
        inner.set_state(SubscriptionState::Stopped {
            reason: "shutdown".to_owned(),
        });
    };
    let mut delay = FIRST_RESTART_DELAY;
    loop {
        let running = match handle.take() {
            Some(running) => running,
            None => loop {
                tokio::select! {
                    _ = stop.changed() => {
                        stopped();
                        return;
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
                delay = (delay * 2).min(MAX_RESTART_DELAY);
                if inner.executor.get().is_none() {
                    match connect_client(&inner.config).await {
                        Ok(client) => {
                            inner.set_executor(client);
                            tracing::info!(addr = %inner.config.addr, "store connected");
                        }
                        Err(err) => {
                            let error = format!("{err:#}");
                            tracing::warn!(addr = %inner.config.addr, %error, "the store cannot be reached yet");
                            inner.set_state(SubscriptionState::Connecting { error });
                            continue;
                        }
                    }
                }
                match inner.start_subscription().await {
                    Ok(running) => break running,
                    Err(err) => {
                        let error = format!("{err:#}");
                        tracing::warn!(%error, "the catalog subscription cannot start yet");
                        inner.set_state(SubscriptionState::Restarting { error });
                    }
                }
            },
        };
        delay = FIRST_RESTART_DELAY;
        tracing::info!("the catalog subscription is running");

        let reason = tokio::select! {
            _ = stop.changed() => {
                running.shutdown().await.ok();
                stopped();
                return;
            }
            reason = running.stopped() => reason,
        };
        if !reason.is_failure() {
            inner.set_state(SubscriptionState::Stopped {
                reason: format!("{reason:?}"),
            });
            return;
        }
        let error = reason
            .error()
            .map(|err| format!("{err:#}"))
            .unwrap_or_else(|| format!("{reason:?}"));
        tracing::warn!(%error, "the catalog subscription stopped, restarting");
        inner.set_state(SubscriptionState::Restarting { error });
    }
}
