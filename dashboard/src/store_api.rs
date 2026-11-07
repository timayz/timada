//! The store API: the listener a merchant's storefront connects to with one
//! of its tenant's API keys, to read and write that tenant's events.
//!
//! Every connection opens with the key's token. The resolver authenticates
//! it against the platform's streams and serves the connection in the key's
//! tenant and nothing else: every request on it acts in that tenant, whatever
//! it names, and the subscriber calls are refused (a storefront keeps its
//! cursors in a store of its own). Read-only unless the key holds
//! `store:write` (see [`read_only_store`](crate::read_only_store)). Revoking
//! a key or changing its scopes ends its live connections through
//! [`StoreConnections`] (a reconnect gets the new scopes); suspending the
//! tenant ends every connection of the tenant, and their reconnects are
//! refused. Each node holds its own connections: the page that took the
//! request ends its node's at once, and [`closing_subscription`], started
//! before the listener, the other nodes'.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use evento::metadata::Event;
use evento::remote::{
    Hello, Rejection, Resolver, ServerHandle, Session, serve_tls_with, serve_with,
};
use evento::subscription::{Context, SubscriptionBuilder};
use timada_tenancy::aggregator::Suspended;
use timada_tenancy::aggregator::api_key::{Revoked, Scoped};
use timada_tenancy::{
    Command, SCOPE_STORE_CHECKOUT, SCOPE_STORE_READ, SCOPE_STORE_WRITE, TenancyError,
};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::{self, ServerConfig};

use crate::cluster::Executor;
use crate::config::StoreApiConfig;

/// One live connection: its key, its tenant, and how to end it.
struct Connection {
    key_id: String,
    tenant_id: String,
    close: watch::Sender<bool>,
}

/// This node's live store connections, so that revoking a key or
/// suspending a tenant ends them now rather than at their next reconnect,
/// and the pages can count them. A key or a tenant cut off on another node
/// is ended here by [`closing_subscription`].
#[derive(Default)]
pub struct StoreConnections {
    live: Mutex<Vec<Connection>>,
}

impl StoreConnections {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Ends every connection opened with `key_id`; how many there were.
    /// After a revocation, or a scope change so the store reconnects under
    /// the new scopes.
    pub fn close_key(&self, key_id: &str) -> usize {
        self.close_where(|c| c.key_id == key_id)
    }

    /// Ends every connection of the tenant; how many there were. After a
    /// suspension: their reconnects are refused.
    pub fn close_tenant(&self, tenant_id: &str) -> usize {
        self.close_where(|c| c.tenant_id == tenant_id)
    }

    fn close_where(&self, pick: impl Fn(&Connection) -> bool) -> usize {
        let mut live = self.live.lock().expect("live poisoned");
        let mut closed = 0;
        for connection in live.iter().filter(|c| pick(c)) {
            let _ = connection.close.send(true);
            closed += 1;
        }
        live.retain(|c| !pick(c));
        closed
    }

    /// How many connections `key_id` has open (the integration test's view).
    #[allow(dead_code)]
    pub fn count(&self, key_id: &str) -> usize {
        self.live
            .lock()
            .expect("live poisoned")
            .iter()
            .filter(|c| c.key_id == key_id)
            .count()
    }

    /// How many storefronts the tenant has connected to this node.
    pub fn count_tenant(&self, tenant_id: &str) -> usize {
        self.live
            .lock()
            .expect("live poisoned")
            .iter()
            .filter(|c| c.tenant_id == tenant_id)
            .count()
    }

    /// How many connections this node serves, by tenant.
    pub fn by_tenant(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for connection in self.live.lock().expect("live poisoned").iter() {
            *counts.entry(connection.tenant_id.clone()).or_insert(0) += 1;
        }
        counts
    }

    fn register(
        self: &Arc<Self>,
        key_id: &str,
        tenant_id: &str,
    ) -> (ConnectionGuard, watch::Receiver<bool>) {
        let (close_tx, close_rx) = watch::channel(false);
        self.live.lock().expect("live poisoned").push(Connection {
            key_id: key_id.to_owned(),
            tenant_id: tenant_id.to_owned(),
            close: close_tx.clone(),
        });
        let guard = ConnectionGuard {
            connections: Arc::clone(self),
            tenant_id: tenant_id.to_owned(),
            close_tx,
        };
        (guard, close_rx)
    }
}

/// What a connection keeps for its lifetime: its entry in the registry,
/// released when the connection ends.
struct ConnectionGuard {
    connections: Arc<StoreConnections>,
    tenant_id: String,
    close_tx: watch::Sender<bool>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.connections
            .live
            .lock()
            .expect("live poisoned")
            .retain(|c| !c.close.same_channel(&self.close_tx));
        tracing::info!(tenant = %self.tenant_id, "store disconnected");
    }
}

/// Turns a key's token into a session in the key's tenant, for one
/// connection.
struct StoreResolver {
    store: Executor,
    connections: Arc<StoreConnections>,
}

#[async_trait::async_trait]
impl Resolver for StoreResolver {
    type Executor = Executor;

    async fn resolve(&self, hello: &Hello) -> Result<Session<Executor>, Rejection> {
        let Some(token) = hello.token.as_deref() else {
            tracing::warn!(peer = %hello.peer, "store connection refused: no key");
            return Err(Rejection::Unauthorized);
        };
        let access = match Command(self.store.clone())
            .authenticate_api_key(token)
            .await
        {
            Ok(access) => access,
            Err(TenancyError::ApiKeyInvalid) => {
                tracing::warn!(peer = %hello.peer, "store connection refused: unknown or revoked key");
                return Err(Rejection::Unauthorized);
            }
            Err(TenancyError::Suspended) => {
                tracing::info!(peer = %hello.peer, "store connection refused: tenant suspended");
                return Err(Rejection::Unavailable("this tenant is suspended".into()));
            }
            Err(err) => {
                tracing::error!(peer = %hello.peer, %err, "store connection failed to authenticate");
                return Err(Rejection::Unavailable(
                    "the store is unavailable, try again later".into(),
                ));
            }
        };
        if !access.key.allows(SCOPE_STORE_READ) {
            tracing::warn!(peer = %hello.peer, key = %access.key.hint, "store connection refused: the key may not read");
            return Err(Rejection::Unauthorized);
        }
        let can_write = access.key.allows(SCOPE_STORE_WRITE);
        let can_sell = access.key.allows(SCOPE_STORE_CHECKOUT);
        let executor = if can_write {
            self.store.clone()
        } else if can_sell {
            crate::read_only_store::checkout_only(self.store.clone())
        } else {
            crate::read_only_store::read_only(self.store.clone())
        };
        let session = Session::new(executor)
            .tenant(&access.tenant.id)
            .map_err(|err| {
                tracing::error!(tenant = %access.tenant.slug, %err, "store connection refused: bad tenant id");
                Rejection::Unavailable("the store is unavailable, try again later".into())
            })?;
        let (guard, close) = self.connections.register(&access.key.id, &access.tenant.id);
        tracing::info!(
            tenant = %access.tenant.slug,
            key = %access.key.hint,
            can_write,
            can_sell,
            peer = %hello.peer,
            "store connected"
        );
        Ok(session.keep(guard).close_when(close))
    }
}

/// The label of [`closing_subscription`]; nothing is stored under it.
pub const CLOSING_SUBSCRIPTION: &str = "timada-dashboard/store-closing";

/// How long before its start [`closing_subscription`] still acts on an
/// event: room for the nodes' clocks to disagree (the cluster's own margins
/// assume they agree to within a fraction of a second).
const CLOSING_MARGIN: Duration = Duration::from_secs(10);

/// Ends this node's connections of a key revoked or re-scoped, and of a
/// tenant suspended, whichever node took the request: each node holds its
/// own connections, and a key is only checked when a connection opens. The
/// page that took the request ends its own node's at once; this ends the
/// others'. It runs on every node, follows every routing key and claims
/// nothing, with its cursor in memory, from the head of the store.
///
/// Start it before the store API takes a connection: every connection is
/// then younger than it, and an event written before it started (less
/// [`CLOSING_MARGIN`]) is already in what the connection's key was checked
/// against, so it is skipped. Right after a whole cluster restarts, the
/// store does not know its head yet and the subscription reads from the
/// beginning; the skip keeps a key re-scoped long ago from dropping
/// connections opened since.
pub fn closing_subscription<E: evento::Executor>(
    connections: Arc<StoreConnections>,
) -> SubscriptionBuilder<E> {
    let since = SystemTime::now()
        .checked_sub(CLOSING_MARGIN)
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |at| u64::try_from(at.as_micros()).unwrap_or(u64::MAX));
    closing_handlers(connections, since)
        .ephemeral()
        .start_from_latest()
}

/// The handlers of [`closing_subscription`], acting on events stamped from
/// `since_micros` (unix microseconds) on.
fn closing_handlers<E: evento::Executor>(
    connections: Arc<StoreConnections>,
    since_micros: u64,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CLOSING_SUBSCRIPTION)
        .data(Closing {
            connections,
            since_micros,
        })
        .handler(on_key_revoked())
        .handler(on_key_scoped())
        .handler(on_tenant_suspended())
        .any_routing_key()
}

/// What the closing handlers share: this node's connections, and the stamp
/// before which an event changes nothing.
#[derive(Clone)]
struct Closing {
    connections: Arc<StoreConnections>,
    since_micros: u64,
}

impl Closing {
    /// The node's connections, unless `event` was written before the
    /// subscription started.
    fn connections_for(&self, event: &evento::Event) -> Option<&StoreConnections> {
        (event.timestamp_micros() >= self.since_micros).then_some(&*self.connections)
    }
}

#[evento::subscription]
async fn on_key_revoked<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<Revoked>,
) -> anyhow::Result<()> {
    let closing: Closing = context.extract();
    let Some(connections) = closing.connections_for(&event) else {
        return Ok(());
    };
    let ended = connections.close_key(&event.aggregate_id);
    if ended > 0 {
        tracing::info!(
            connections = ended,
            "store connections ended by a key revocation"
        );
    }
    Ok(())
}

#[evento::subscription]
async fn on_key_scoped<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<Scoped>,
) -> anyhow::Result<()> {
    let closing: Closing = context.extract();
    let Some(connections) = closing.connections_for(&event) else {
        return Ok(());
    };
    let ended = connections.close_key(&event.aggregate_id);
    if ended > 0 {
        tracing::info!(
            connections = ended,
            "store connections ended by a key scope change"
        );
    }
    Ok(())
}

#[evento::subscription]
async fn on_tenant_suspended<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<Suspended>,
) -> anyhow::Result<()> {
    let closing: Closing = context.extract();
    let Some(connections) = closing.connections_for(&event) else {
        return Ok(());
    };
    let ended = connections.close_tenant(&event.aggregate_id);
    if ended > 0 {
        tracing::info!(
            tenant = %event.aggregate_id,
            connections = ended,
            "store connections ended by a suspension"
        );
    }
    Ok(())
}

/// Binds the store API and serves it; `None` when the section is absent.
pub async fn serve(
    config: Option<&StoreApiConfig>,
    store: Executor,
    connections: Arc<StoreConnections>,
) -> Result<Option<ServerHandle>> {
    let Some(config) = config else {
        return Ok(None);
    };
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("cannot bind the store API on {}", config.listen))?;
    let addr = listener.local_addr()?;
    let resolver = StoreResolver { store, connections };
    let handle = match config.tls() {
        Some((cert, key)) => {
            let acceptor = tls_acceptor(cert, key)?;
            tracing::info!(%addr, "store API listening (TLS)");
            serve_tls_with(listener, acceptor, resolver)
        }
        None => {
            tracing::info!(%addr, "store API listening (no encryption)");
            serve_with(listener, resolver)
        }
    };
    Ok(Some(handle))
}

/// The server side of TLS from PEM files: the certificate chain and its key.
fn tls_acceptor(cert: &Path, key: &Path) -> Result<TlsAcceptor> {
    // rustls needs a process-wide crypto provider; installing twice is fine.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .with_context(|| format!("cannot read {}", cert.display()))?
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("cannot parse {}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key)
        .with_context(|| format!("cannot read {}", key.display()))?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("the store API certificate and key do not go together")?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store holding one scope change of key `k1`, written now.
    async fn store_with_a_scope_change() -> evento::Turso {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        evento::append("k1")
            .original_version(0)
            .event(&Scoped {
                scopes: vec![SCOPE_STORE_READ.to_owned()],
                scoped_by: "alice".to_owned(),
                scoped_at: 0,
            })
            .commit(&store)
            .await
            .unwrap();
        store
    }

    #[tokio::test]
    async fn an_event_from_before_the_start_ends_no_connection() {
        let store = store_with_a_scope_change().await;
        let connections = StoreConnections::new();
        let (_guard, closed) = connections.register("k1", "acme");

        // Read from the beginning, as when the store does not know its head
        // yet: the change is older than the start, so the key's connection
        // already carries it.
        closing_handlers::<evento::Turso>(connections.clone(), u64::MAX)
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        assert_eq!(connections.count("k1"), 1);
        assert!(!*closed.borrow());

        // Started before it, the change ends the connection.
        closing_handlers::<evento::Turso>(connections.clone(), 0)
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        assert_eq!(connections.count("k1"), 0);
        assert!(*closed.borrow());
    }
}
