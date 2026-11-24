//! Write side: the command gateway and the strict write model it loads.
//!
//! The write model folds *all* events of a connection (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never
//! goes stale. It is not snapshotted: a connection is a handful of events.
//!
//! A connection's stream is written by the merchant and by the host at the
//! same time (a site chosen while a pass notices the grant lapsed), so a
//! write that lost to another is no verdict: every command loads the
//! connection again and decides again ([`WRITE_ATTEMPTS`] times).

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{Connected, Connection, Disconnected, Lapsed, SiteChosen};
use crate::error::GoogleError;
use crate::value_object::{
    ACCESS_SEARCH_CONSOLE, ACCESSES, MAX_ACCOUNT_LEN, connection_id, normalize_site,
};

/// The command gateway over the platform's streams. Cheap to clone when `E`
/// is (the dashboard's executor is an `Arc` wrapper).
pub struct Command<E: Executor>(pub E);

/// How many times a command decides again when another write to the
/// connection came in between its load and its write.
const WRITE_ATTEMPTS: usize = 5;

/// The write model of a shop's connection.
#[evento::projection(id = id, name = "timada-google/ConnectionState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ConnectionState {
    pub id: String,
    /// The shop.
    pub tenant_id: String,
    /// Whether a grant is held: `false` once the merchant took it off.
    pub connected: bool,
    /// What renews the access, sealed by the host; empty when not
    /// connected.
    pub refresh_token_sealed: Vec<u8>,
    /// The Google account that granted it.
    pub account: String,
    /// The kinds of access it covers.
    pub accesses: Vec<String>,
    /// The shop's Search Console site, once chosen.
    pub site: Option<String>,
    /// Google no longer honours the grant: the merchant has to connect
    /// again.
    pub lapsed: bool,
    /// Unix seconds of the grant held.
    pub connected_at: u64,
}

impl ConnectionState {
    /// Whether the grant covers `access` (one of [`ACCESSES`]).
    pub fn covers(&self, access: &str) -> bool {
        self.connected && self.accesses.iter().any(|held| held == access)
    }

    /// Whether Google can be read through the grant for `access`: held,
    /// covering it, and not lapsed.
    pub fn reads(&self, access: &str) -> bool {
        self.covers(access) && !self.lapsed
    }
}

pub fn connection_projection<E: Executor>() -> Projection<E, ConnectionState> {
    Projection::new::<Connection>()
        .handler(on_connected())
        .handler(on_site_chosen())
        .handler(on_lapsed())
        .handler(on_disconnected())
        .strict()
}

#[evento::handler]
async fn on_connected(
    event: Event<Connected>,
    connection: &mut ConnectionState,
) -> anyhow::Result<()> {
    connection.id = event.aggregate_id.to_owned();
    connection.tenant_id = event.data.tenant_id.to_owned();
    // Another account sees other sites, and a grant without Search Console
    // sees none: the site chosen goes with either.
    let keeps_site = connection.connected
        && connection.account == event.data.account
        && event
            .data
            .accesses
            .iter()
            .any(|access| access == ACCESS_SEARCH_CONSOLE);
    if !keeps_site {
        connection.site = None;
    }
    connection.connected = true;
    connection.refresh_token_sealed = event.data.refresh_token_sealed.to_owned();
    connection.account = event.data.account.to_owned();
    connection.accesses = event.data.accesses.to_owned();
    connection.lapsed = false;
    connection.connected_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_site_chosen(
    event: Event<SiteChosen>,
    connection: &mut ConnectionState,
) -> anyhow::Result<()> {
    connection.site = Some(event.data.site.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_lapsed(event: Event<Lapsed>, connection: &mut ConnectionState) -> anyhow::Result<()> {
    let _ = event;
    connection.lapsed = true;
    Ok(())
}

#[evento::handler]
async fn on_disconnected(
    event: Event<Disconnected>,
    connection: &mut ConnectionState,
) -> anyhow::Result<()> {
    *connection = ConnectionState {
        id: event.aggregate_id.to_owned(),
        tenant_id: connection.tenant_id.clone(),
        aggregate_version: connection.aggregate_version,
        ..ConnectionState::default()
    };
    Ok(())
}

/// What a Google account granted, as the host got it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    /// What renews the access, sealed by the host for the shop.
    pub refresh_token_sealed: Vec<u8>,
    /// The account's e-mail address.
    pub account: String,
    /// The kinds of access the account granted.
    pub accesses: Vec<String>,
}

impl<E: Executor> Command<E> {
    /// A shop's connection as it stands; `None` for a shop that never had
    /// one. One taken off reads as not connected.
    pub async fn load_connection(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<Option<ConnectionState>> {
        connection_projection()
            .load(connection_id(tenant_id))
            .execute(&self.0)
            .await
    }

    /// Loads the connection, lets `decide` say what to write, and writes
    /// it; again when another write came in between. `decide` returns
    /// `None` when there is nothing to write.
    async fn settle<F>(
        &self,
        tenant_id: &str,
        routing_key: Option<String>,
        decide: F,
    ) -> Result<bool, GoogleError>
    where
        F: Fn(Option<&ConnectionState>, &mut evento::WriteBuilder) -> Result<bool, GoogleError>,
    {
        for _ in 0..WRITE_ATTEMPTS {
            let loaded = self.load_connection(tenant_id).await?;
            let mut write = match &loaded {
                Some(connection) => connection.write()?,
                None => {
                    let mut write = evento::append(connection_id(tenant_id));
                    write.original_version(0);
                    write
                }
            };
            if !decide(loaded.as_ref(), &mut write)? {
                return Ok(false);
            }
            write.routing_key_opt(routing_key.clone());
            match write.commit(&self.0).await {
                Ok(_) => return Ok(true),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Records what a Google account granted the shop, in place of the
    /// grant held before. The site chosen stays when the same account
    /// grants Search Console again.
    pub async fn connect(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: Connect,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        if cmd.accesses.is_empty() {
            return Err(GoogleError::NothingGranted);
        }
        if cmd
            .accesses
            .iter()
            .any(|access| !ACCESSES.contains(&access.as_str()))
        {
            return Err(GoogleError::UnknownAccess);
        }
        let connected = Connected {
            tenant_id: tenant_id.to_owned(),
            refresh_token_sealed: cmd.refresh_token_sealed,
            account: cmd.account.trim().chars().take(MAX_ACCOUNT_LEN).collect(),
            accesses: ACCESSES
                .into_iter()
                .filter(|access| cmd.accesses.iter().any(|asked| asked == access))
                .map(str::to_owned)
                .collect(),
            connected_by: actor_user_id.to_owned(),
        };
        self.settle(tenant_id, routing_key, |_, write| {
            write.event(&connected);
            Ok(true)
        })
        .await?;
        Ok(())
    }

    /// Says which of the account's Search Console sites is the shop's; the
    /// same again is a no-op. The host checks beforehand that the account
    /// sees it.
    pub async fn choose_site(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        site: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let site = normalize_site(site)?;
        self.settle(tenant_id, routing_key, |connection, write| {
            let connection = connection
                .filter(|connection| connection.connected)
                .ok_or(GoogleError::NotConnected)?;
            if connection.lapsed {
                return Err(GoogleError::Lapsed);
            }
            if !connection.covers(ACCESS_SEARCH_CONSOLE) {
                return Err(GoogleError::NoSearchConsole);
            }
            if connection.site.as_deref() == Some(site.as_str()) {
                return Ok(false);
            }
            write.event(&SiteChosen {
                site: site.clone(),
                chosen_by: actor_user_id.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }

    /// Records that Google refused to renew the access under
    /// `refresh_token_sealed`. Whether it was written: a grant replaced
    /// since, taken off, or known as lapsed already is left alone.
    pub async fn note_lapsed(
        &self,
        actor: &str,
        tenant_id: &str,
        refresh_token_sealed: &[u8],
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        self.settle(tenant_id, routing_key, |connection, write| {
            let current = connection.is_some_and(|connection| {
                connection.connected
                    && !connection.lapsed
                    && connection.refresh_token_sealed == refresh_token_sealed
            });
            if !current {
                return Ok(false);
            }
            write.event(&Lapsed {
                noticed_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }

    /// Forgets the shop's grant; none is a no-op.
    pub async fn disconnect(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        self.settle(tenant_id, routing_key, |connection, write| {
            if !connection.is_some_and(|connection| connection.connected) {
                return Ok(false);
            }
            write.event(&Disconnected {
                disconnected_by: actor_user_id.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }
}
