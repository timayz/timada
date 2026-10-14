//! The store API: the listener a merchant's storefront connects to with one
//! of its tenant's API keys, to read and write that tenant's events.
//!
//! Every connection opens with the key's token. The resolver authenticates
//! it against the platform store, leases the tenant (opening its database
//! if needed: a store connection is tenant activity) and serves the
//! connection with that tenant's executor and nothing else, read-only unless
//! the key holds `store:write` (see [`read_only_store`](crate::read_only_store)). Revoking a key or
//! changing its scopes ends its live connections through
//! [`StoreConnections`] (a reconnect gets the new scopes); suspending the
//! tenant closes its database, which breaks them, and their reconnects are
//! refused.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use evento::remote::{
    Hello, Rejection, Resolver, ServerHandle, Session, serve_tls_with, serve_with,
};
use timada_tenancy::{Command, SCOPE_STORE_READ, SCOPE_STORE_WRITE, TenancyError};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::{self, ServerConfig};

use crate::cluster::Executor;
use crate::config::StoreApiConfig;
use crate::tenants::{TenantLease, Tenants};

/// The live store connections by key id, so that revoking a key ends them
/// now rather than at their next reconnect.
#[derive(Default)]
pub struct StoreConnections {
    by_key: Mutex<HashMap<String, Vec<watch::Sender<bool>>>>,
}

impl StoreConnections {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Ends every connection opened with `key_id`; how many there were.
    /// After a revocation, or a scope change so the store reconnects under
    /// the new scopes.
    pub fn close_key(&self, key_id: &str) -> usize {
        let senders = self
            .by_key
            .lock()
            .expect("by_key poisoned")
            .remove(key_id)
            .unwrap_or_default();
        for close in &senders {
            let _ = close.send(true);
        }
        senders.len()
    }

    /// How many connections `key_id` has open (the integration test's view).
    #[allow(dead_code)]
    pub fn count(&self, key_id: &str) -> usize {
        self.by_key
            .lock()
            .expect("by_key poisoned")
            .get(key_id)
            .map_or(0, Vec::len)
    }

    fn register(
        self: &Arc<Self>,
        key_id: &str,
        lease: TenantLease,
    ) -> (ConnectionGuard, watch::Receiver<bool>) {
        let (close_tx, close_rx) = watch::channel(false);
        self.by_key
            .lock()
            .expect("by_key poisoned")
            .entry(key_id.to_owned())
            .or_default()
            .push(close_tx.clone());
        let guard = ConnectionGuard {
            connections: Arc::clone(self),
            key_id: key_id.to_owned(),
            lease,
            close_tx,
        };
        (guard, close_rx)
    }
}

/// What a connection keeps for its lifetime: the tenant lease and its entry
/// in the registry, both released when the connection ends.
struct ConnectionGuard {
    connections: Arc<StoreConnections>,
    key_id: String,
    lease: TenantLease,
    close_tx: watch::Sender<bool>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut by_key = self.connections.by_key.lock().expect("by_key poisoned");
        if let Some(senders) = by_key.get_mut(&self.key_id) {
            senders.retain(|s| !s.same_channel(&self.close_tx));
            if senders.is_empty() {
                by_key.remove(&self.key_id);
            }
        }
        tracing::info!(tenant = %self.lease.tenant_id(), "store disconnected");
    }
}

/// Turns a key's token into the tenant's executor, for one connection.
struct StoreResolver {
    platform: Executor,
    tenants: Arc<Tenants>,
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
        let access = match Command(self.platform.clone())
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
        let lease = self.tenants.lease(&access.tenant).await.map_err(|err| {
            tracing::warn!(tenant = %access.tenant.slug, %err, "store connection refused: tenant not opened");
            Rejection::Unavailable("this tenant's store cannot be opened right now".into())
        })?;
        let can_write = access.key.allows(SCOPE_STORE_WRITE);
        let executor = if can_write {
            lease.executor().clone()
        } else {
            crate::read_only_store::read_only(lease.executor().clone(), &access.key.id)
        };
        let (guard, close) = self.connections.register(&access.key.id, lease);
        tracing::info!(
            tenant = %access.tenant.slug,
            key = %access.key.hint,
            can_write,
            peer = %hello.peer,
            "store connected"
        );
        Ok(Session::new(executor).keep(guard).close_when(close))
    }
}

/// Binds the store API and serves it; `None` when the section is absent.
pub async fn serve(
    config: Option<&StoreApiConfig>,
    platform: Executor,
    tenants: Arc<Tenants>,
    connections: Arc<StoreConnections>,
) -> Result<Option<ServerHandle>> {
    let Some(config) = config else {
        return Ok(None);
    };
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("cannot bind the store API on {}", config.listen))?;
    let addr = listener.local_addr()?;
    let resolver = StoreResolver {
        platform,
        tenants,
        connections,
    };
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
