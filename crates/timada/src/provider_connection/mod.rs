//! Provider connections: a store's configured account with a provider.
//!
//! One aggregate instance per connected account. The provider *implementation*
//! lives in the host's `ProviderRegistry`; this aggregate owns the runtime
//! data: which kind, its credentials, and whether it is enabled.
//!
//! Credentials live only in the event store and in this write-side state —
//! they are deliberately never copied into a read-model table.

use anyhow::Result;
use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use evento::sql::RwSqlite;
use timada_provider::ProviderRegistry;

#[evento::aggregate]
pub enum ProviderConnection {
    /// The store connected a new provider account.
    ProviderConnected {
        provider_kind: String,
        display_name: String,
    },
    /// Credentials/settings for the connection were configured.
    /// The config is a full replacement, keyed by the provider's
    /// `ConfigField::key`s.
    ProviderCredentialsConfigured { config: Vec<(String, String)> },
    ProviderEnabled,
    ProviderDisabled,
}

/// Write-side state, replayed from the connection's events.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug)]
pub struct ProviderConnectionState {
    pub id: String,
    pub provider_kind: String,
    pub display_name: String,
    pub config: Vec<(String, String)>,
    pub enabled: bool,
}

impl ProjectionAggregate for ProviderConnectionState {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn on_provider_connected(
    event: Event<ProviderConnected>,
    state: &mut ProviderConnectionState,
) -> Result<()> {
    state.id = event.aggregate_id.clone();
    state.provider_kind = event.data.provider_kind.clone();
    state.display_name = event.data.display_name.clone();
    Ok(())
}

#[evento::handler]
async fn on_provider_credentials_configured(
    event: Event<ProviderCredentialsConfigured>,
    state: &mut ProviderConnectionState,
) -> Result<()> {
    state.config = event.data.config.clone();
    Ok(())
}

#[evento::handler]
async fn on_provider_enabled(
    _event: Event<ProviderEnabled>,
    state: &mut ProviderConnectionState,
) -> Result<()> {
    state.enabled = true;
    Ok(())
}

#[evento::handler]
async fn on_provider_disabled(
    _event: Event<ProviderDisabled>,
    state: &mut ProviderConnectionState,
) -> Result<()> {
    state.enabled = false;
    Ok(())
}

fn state_projection() -> Projection<RwSqlite, ProviderConnectionState> {
    Projection::new::<ProviderConnection>()
        .handler(on_provider_connected())
        .handler(on_provider_credentials_configured())
        .handler(on_provider_enabled())
        .handler(on_provider_disabled())
        .strict()
}

/// Load the write-side state of one connection.
///
/// Public because credentials never reach a read-model table: callers that
/// need config values (provider API calls, the admin credentials form) must
/// read them from here.
pub async fn load(executor: &RwSqlite, id: &str) -> Result<Option<ProviderConnectionState>> {
    state_projection().load(id).execute(executor).await
}

/// Connect a new provider account. The kind must be registered in the host's
/// [`ProviderRegistry`]. Returns the new connection's id.
#[tracing::instrument(skip_all, fields(provider_kind))]
pub async fn connect_provider(
    executor: &RwSqlite,
    registry: &ProviderRegistry,
    provider_kind: &str,
) -> Result<String> {
    let Some(provider) = registry.get(provider_kind) else {
        anyhow::bail!("unknown provider kind: {provider_kind}");
    };

    let id = evento::create()
        .event(&ProviderConnected {
            provider_kind: provider_kind.to_owned(),
            display_name: provider.display_name().to_owned(),
        })
        .commit(executor)
        .await?;

    tracing::info!(aggregate_id = %id, "provider connected");
    Ok(id)
}

/// Replace the connection's credentials/settings. An empty config clears them
/// (and [`enable_provider`] will refuse until they are configured again).
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn configure_provider_credentials(
    executor: &RwSqlite,
    id: &str,
    config: Vec<(String, String)>,
) -> Result<()> {
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("provider connection not found: {id}");
    };

    state
        .write()?
        .event(&ProviderCredentialsConfigured { config })
        .commit(executor)
        .await?;

    tracing::info!("provider credentials configured");
    Ok(())
}

/// Enable the connection. Refused while the provider's config schema is
/// non-empty but no credentials are configured.
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn enable_provider(
    executor: &RwSqlite,
    registry: &ProviderRegistry,
    id: &str,
) -> Result<()> {
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("provider connection not found: {id}");
    };
    if state.enabled {
        anyhow::bail!("provider connection is already enabled");
    }
    let Some(provider) = registry.get(&state.provider_kind) else {
        anyhow::bail!("unknown provider kind: {}", state.provider_kind);
    };
    if !provider.config_schema().is_empty() && state.config.is_empty() {
        anyhow::bail!("cannot enable a provider before its credentials are configured");
    }

    state
        .write()?
        .event(&ProviderEnabled)
        .commit(executor)
        .await?;

    tracing::info!("provider enabled");
    Ok(())
}

/// Disable the connection.
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn disable_provider(executor: &RwSqlite, id: &str) -> Result<()> {
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("provider connection not found: {id}");
    };
    if !state.enabled {
        anyhow::bail!("provider connection is already disabled");
    }

    state
        .write()?
        .event(&ProviderDisabled)
        .commit(executor)
        .await?;

    tracing::info!("provider disabled");
    Ok(())
}
