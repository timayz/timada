//! The connection to the store and the local copy of its catalog.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use evento::remote::ClientState;
use evento::subscription::{State, Subscription};
use evento::{Evento, Executor as _, RemoteClient, Sidecar};
use timada_core::db::Db;

use crate::config::StoreConfig;
use crate::db::StoreDb;
use crate::error::StoreUnavailable;
use crate::money::MoneyFormat;
use crate::read_model::catalog_subscription;

/// How long the catalog subscription waits before it goes on after a pass
/// that failed, doubling up to [`MAX_RESTART_DELAY`] while it fails again.
const FIRST_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);

/// How often [`first_answer`] looks at where the client stands.
const FIRST_ANSWER_POLL: Duration = Duration::from_millis(20);

/// A connected store. Cheap to clone; every clone shares the connection, the
/// local database and the catalog subscription.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    config: StoreConfig,
    /// The connection to the store API. It exists whether the store answers
    /// or not: it dials in the background, again after a growing delay with
    /// some chance in it, and says how it stands.
    client: RemoteClient,
    /// The store through that connection: events are read from and written
    /// to it, cursors and snapshots stay local.
    executor: Evento,
    /// Whether the store answered since the shop started.
    answered: AtomicBool,
    db: StoreDb,
    /// The dashboard's HTTP side, when the host gave its address.
    api: Option<crate::reviews::Api>,
    /// The catalog subscription, supervised: a pass that kept failing (the
    /// store was away longer than its retries) is gone on from, at the
    /// local cursor, after a growing delay.
    subscription: Subscription,
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
    /// The store has not answered since the shop started: it is dialled
    /// again after a growing delay, and the pages serve the local copy
    /// meanwhile.
    Connecting {
        error: String,
    },
    Running,
    /// A pass kept failing (the store was unreachable for a while); the
    /// subscription goes on from its local cursor after a growing delay.
    Restarting {
        error: String,
    },
    /// Ended: on shutdown, or because the store refuses the key, which no
    /// waiting cures.
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
    /// A store that cannot be reached does not, nor one that says it cannot
    /// serve the key's shop right now (a suspended shop, a store that could
    /// not check the key: `Rejection::Unavailable`, what a degraded cluster
    /// answers): the shop opens on the catalog it has (empty, on a first
    /// start), the store is dialled again after a growing delay, and until it
    /// answers every call that needs it fails with
    /// [`StoreUnavailable`](crate::StoreUnavailable);
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
        let client = client_of(&config);
        // The client carries the store's routing key (told by the server's
        // first Hello); the wrapper asks it, so it prefixes the cursor key
        // and keys new aggregates once the store has answered.
        let executor = Evento::new(Sidecar::from((client.clone(), db.store().clone())));
        let answered = match first_answer(&client, config.request_timeout).await {
            Ok(true) => {
                tracing::info!(addr = %config.addr, "store connected");
                true
            }
            Ok(false) => {
                tracing::warn!(
                    addr = %config.addr,
                    error = %connecting_error(&client.state()),
                    "the store cannot be reached: serving the local copy until it answers"
                );
                false
            }
            Err(err) => {
                return Err(err.context(format!("cannot connect to the store at {}", config.addr)));
            }
        };
        // Supervised, it starts whether the store answers or not, and
        // handles its first event when it does.
        let subscription = catalog_subscription(db.db().clone())
            .supervised()
            .restart_backoff(FIRST_RESTART_DELAY, MAX_RESTART_DELAY)
            .start(&executor)
            .await
            .context("cannot start following the catalog")?;
        Ok(Store {
            inner: Arc::new(Inner {
                config,
                client,
                executor,
                answered: AtomicBool::new(answered),
                db,
                api,
                subscription,
            }),
        })
    }

    /// The store's executor: events are read from and written to the store,
    /// subscription cursors and snapshots stay local. Fails while the store
    /// has not answered since the shop started.
    pub fn executor(&self) -> Result<&Evento, StoreUnavailable> {
        if self.inner.has_answered() {
            Ok(&self.inner.executor)
        } else {
            Err(StoreUnavailable::new("the store API has not answered yet"))
        }
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

    /// The address shoppers reach the shop at, when the host said so.
    pub fn public_url(&self) -> Option<&str> {
        self.inner.config.public_url.as_deref()
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
        // Not asked of a store that never answered: the call would only say
        // so.
        let connected = if self.inner.has_answered() {
            let probe = self
                .inner
                .executor
                .latest_timestamp(None, None, evento::TenantFilter::All);
            tokio::time::timeout(self.inner.config.request_timeout, probe)
                .await
                .map(|answer| answer.is_ok())
                .unwrap_or(false)
        } else {
            false
        };
        let db = self.inner.db.db();
        // Kept as the whole shop's page lists and unlists them.
        let products: i64 = db
            .fetch_optional::<(i64,)>("SELECT products FROM storefront_shop", ())
            .await
            .ok()
            .flatten()
            .map_or(0, |(products,)| products);
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
            subscription: self.inner.subscription_state(),
            products,
            last_event_at,
            lag_seconds,
        }
    }

    /// Stops following the catalog, once the subscription is done with the
    /// event in hand. The local database is left as it is and resumes from
    /// its cursor on the next connection.
    pub async fn shutdown(self) {
        self.inner.subscription.stop();
        self.inner.subscription.stopped().await;
    }
}

impl Inner {
    /// Whether the store answered since the shop started: seen connected
    /// once is enough, whatever the connection does afterwards.
    fn has_answered(&self) -> bool {
        if self.answered.load(Ordering::Relaxed) {
            return true;
        }
        let connected = self.client.state().is_connected();
        if connected {
            self.answered.store(true, Ordering::Relaxed);
        }
        connected
    }

    /// Where the catalog subscription stands, as the status says it.
    fn subscription_state(&self) -> SubscriptionState {
        let client = self.client.state();
        if matches!(client, ClientState::Refused) {
            return SubscriptionState::Stopped {
                reason: "the store refuses this key".to_owned(),
            };
        }
        if !self.has_answered() {
            return SubscriptionState::Connecting {
                error: connecting_error(&client),
            };
        }
        match self.subscription.state() {
            State::Running => SubscriptionState::Running,
            State::Restarting { error, .. } => SubscriptionState::Restarting {
                error: format!("{error:#}"),
            },
            State::Stopped(reason) => SubscriptionState::Stopped {
                reason: format!("{reason:?}"),
            },
            // A state evento names later: shown as it says it.
            other => SubscriptionState::Stopped {
                reason: other.to_string(),
            },
        }
    }
}

/// The connection to the store API of `config`, dialled in the background:
/// it is there whether the store answers or not.
fn client_of(config: &StoreConfig) -> RemoteClient {
    let mut builder =
        RemoteClient::builder(config.addr.clone()).request_timeout(config.request_timeout);
    builder = builder.token(config.token.clone());
    if let Some(tls) = config.tls.clone() {
        builder = builder.tls(tls.connector, tls.server_name);
    }
    builder.connect_lazy()
}

/// Why the last attempt to reach the store failed, for the status and the
/// log; empty while none has.
fn connecting_error(state: &ClientState) -> String {
    match state {
        ClientState::Connecting {
            error: Some(error), ..
        } => format!("{error:#}"),
        _ => String::new(),
    }
}

/// What the store first says, waited for `within` at most: `true` once it
/// answered, `false` when a first attempt failed in a way that is tried
/// again (unreachable, or it cannot serve the key's shop right now) or
/// nothing came in time, and an error when it refuses the key itself
/// (`Rejection::Unauthorized`): no other answer means the key will never do.
async fn first_answer(client: &RemoteClient, within: Duration) -> anyhow::Result<bool> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match client.state() {
            ClientState::Connected => return Ok(true),
            // Its error carries the store's rejection.
            ClientState::Refused => return client.connected().await.map(|()| true),
            ClientState::Connecting { error: Some(_), .. } => return Ok(false),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(FIRST_ANSWER_POLL).await;
    }
}
