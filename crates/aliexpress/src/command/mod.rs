//! Write side: the link between an AliExpress item and the catalog product
//! it filled, and its history of reads; and (in [`forward`]) the shop's
//! orders placed on AliExpress.

mod forward;

pub use forward::*;

use std::ops::Deref;
use std::time::{SystemTime, UNIX_EPOCH};

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_tenancy::Pricing;

use crate::aggregator::{
    CombinationIgnored, CombinationRestored, CombinationsAdded, CombinationsFound, ImportFailed,
    ImportRequested, ImportStarted, Imported, NewSku, PricingSet, SkuLink, SyncFailed, Synced,
    Unlinked,
};
use crate::error::AliexpressError;
use crate::value_object::{IMPORT_STALE_SECS, aliexpress_product_id};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes, loads or reads is
    /// that tenant's. `tenant` must not be empty (the default tenant holds
    /// the platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// A gateway naming no tenant: for a store that is one tenant's already
    /// (a storefront's connection, which the store API serves in its key's
    /// tenant whatever its requests name). Every call then acts in the
    /// default tenant, which that store maps to its own.
    pub fn unscoped(executor: E) -> Self {
        Self {
            executor,
            tenant: String::new(),
        }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The store the tenant's streams live in.
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// What a read of a linked item recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncRead {
    /// The combinations as now read.
    pub skus: Vec<SkuLink>,
    /// AliExpress no longer sells it.
    pub offline: bool,
    /// What the read found that the shop does not sell, when that changed.
    pub found: Option<Vec<NewSku>>,
}

/// Where an import stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinkStatus {
    /// Asked for, not done yet.
    #[default]
    Importing,
    /// A catalog product is kept in step with it.
    Linked,
    /// Nothing was imported.
    Failed,
    /// No longer kept in step.
    Unlinked,
}

impl LinkStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Importing => "importing",
            Self::Linked => "linked",
            Self::Failed => "failed",
            Self::Unlinked => "unlinked",
        }
    }
}

/// One AliExpress item of the shop.
#[evento::projection(id = id, name = "timada-aliexpress/ProductState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct LinkState {
    pub id: String,
    pub ae_product_id: String,
    pub status: LinkStatus,
    /// Whether a node took the import on.
    pub started: bool,
    /// The catalog product, once imported.
    pub product_id: Option<String>,
    pub skus: Vec<SkuLink>,
    /// What the last read found that the shop does not sell.
    pub found: Vec<NewSku>,
    /// The combinations the merchant does not want offered, by id.
    pub ignored: Vec<String>,
    /// The product's own pricing; `None` follows the shop's.
    pub pricing: Option<Pricing>,
    /// AliExpress no longer sells it (as of the last read).
    pub offline: bool,
    /// Unix seconds of the last read that worked (the import counts).
    pub last_synced_at: Option<u64>,
    /// Why the last import or read failed; cleared by one that worked.
    pub last_error: Option<String>,
    pub requested_by: String,
    /// Unix seconds of the last `ImportRequested`.
    pub requested_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl LinkState {
    pub fn is_linked(&self) -> bool {
        self.status == LinkStatus::Linked
    }

    /// What is offered to the merchant: found, and not ignored.
    pub fn offered(&self) -> impl Iterator<Item = &NewSku> {
        self.found
            .iter()
            .filter(|sku| !self.ignored.contains(&sku.sku_id))
    }

    /// What was found and ignored.
    pub fn ignored_found(&self) -> impl Iterator<Item = &NewSku> {
        self.found
            .iter()
            .filter(|sku| self.ignored.contains(&sku.sku_id))
    }
}

pub fn link_projection<E: Executor>() -> Projection<E, LinkState> {
    Projection::new::<crate::aggregator::AliexpressProduct>()
        .handler(on_import_requested())
        .handler(on_import_started())
        .handler(on_imported())
        .handler(on_import_failed())
        .handler(on_synced())
        .handler(on_sync_failed())
        .handler(on_pricing_set())
        .handler(on_unlinked())
        .handler(on_combinations_found())
        .handler(on_combinations_added())
        .handler(on_combination_ignored())
        .handler(on_combination_restored())
        .strict()
}

#[evento::handler]
async fn on_import_requested(
    event: Event<ImportRequested>,
    link: &mut LinkState,
) -> anyhow::Result<()> {
    link.id = event.aggregate_id.to_owned();
    link.ae_product_id = event.data.ae_product_id.to_owned();
    link.status = LinkStatus::Importing;
    link.started = false;
    link.product_id = None;
    link.skus.clear();
    link.found.clear();
    link.ignored.clear();
    link.offline = false;
    link.last_error = None;
    link.requested_by = event.data.requested_by.to_owned();
    link.requested_at = event.timestamp;
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_import_started(
    event: Event<ImportStarted>,
    link: &mut LinkState,
) -> anyhow::Result<()> {
    link.started = true;
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_imported(event: Event<Imported>, link: &mut LinkState) -> anyhow::Result<()> {
    link.status = LinkStatus::Linked;
    link.product_id = Some(event.data.product_id.to_owned());
    link.skus = event.data.skus.to_owned();
    link.last_synced_at = Some(event.timestamp);
    link.last_error = None;
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_import_failed(event: Event<ImportFailed>, link: &mut LinkState) -> anyhow::Result<()> {
    link.status = LinkStatus::Failed;
    link.last_error = Some(event.data.reason.to_owned());
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_synced(event: Event<Synced>, link: &mut LinkState) -> anyhow::Result<()> {
    link.skus = event.data.skus.to_owned();
    link.offline = event.data.offline;
    link.last_synced_at = Some(event.timestamp);
    link.last_error = None;
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_sync_failed(event: Event<SyncFailed>, link: &mut LinkState) -> anyhow::Result<()> {
    link.last_error = Some(event.data.reason.to_owned());
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_pricing_set(event: Event<PricingSet>, link: &mut LinkState) -> anyhow::Result<()> {
    link.pricing = event.data.pricing.clone();
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_unlinked(event: Event<Unlinked>, link: &mut LinkState) -> anyhow::Result<()> {
    link.status = LinkStatus::Unlinked;
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_combinations_found(
    event: Event<CombinationsFound>,
    link: &mut LinkState,
) -> anyhow::Result<()> {
    link.found = event.data.found.to_owned();
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_combinations_added(
    event: Event<CombinationsAdded>,
    link: &mut LinkState,
) -> anyhow::Result<()> {
    for added in event.data.skus.iter() {
        link.found.retain(|sku| sku.sku_id != added.sku_id);
        link.ignored.retain(|sku_id| sku_id != &added.sku_id);
        link.skus.retain(|sku| sku.sku_id != added.sku_id);
        link.skus.push(added.clone());
    }
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_combination_ignored(
    event: Event<CombinationIgnored>,
    link: &mut LinkState,
) -> anyhow::Result<()> {
    if !link.ignored.contains(&event.data.sku_id) {
        link.ignored.push(event.data.sku_id.to_owned());
    }
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_combination_restored(
    event: Event<CombinationRestored>,
    link: &mut LinkState,
) -> anyhow::Result<()> {
    link.ignored.retain(|sku_id| sku_id != &event.data.sku_id);
    link.updated_at = event.timestamp;
    Ok(())
}

/// Turns a lost race into the variant the caller handles.
fn lost(err: AliexpressError) -> AliexpressError {
    match err {
        AliexpressError::Write(WriteError::InvalidOriginalVersion) => AliexpressError::Importing,
        other => other,
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Asks for `ae_product_id` to be imported; the link's id. An item
    /// imported already is `AlreadyLinked`, one importing `Importing`
    /// (unless that import stalled); a failed or unlinked one is asked for
    /// again.
    pub async fn request_import(
        &self,
        actor: &str,
        ae_product_id: &str,
        routing_key: Option<String>,
    ) -> Result<String, AliexpressError> {
        let requested = ImportRequested {
            ae_product_id: ae_product_id.to_owned(),
            requested_by: actor.to_owned(),
        };
        let id = aliexpress_product_id(ae_product_id);
        match self.load_link(ae_product_id).await? {
            None => {
                evento::append(&id)
                    .original_version(0)
                    .routing_key_opt(routing_key)
                    .event(&requested)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(|err| lost(err.into()))?;
            }
            Some(link) => {
                match link.status {
                    LinkStatus::Linked => return Err(AliexpressError::AlreadyLinked),
                    LinkStatus::Importing
                        if unix_now().saturating_sub(link.updated_at) < IMPORT_STALE_SECS =>
                    {
                        return Err(AliexpressError::Importing);
                    }
                    _ => {}
                }
                link.write()?
                    .routing_key_opt(routing_key)
                    .event(&requested)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(|err| lost(err.into()))?;
            }
        }
        Ok(id)
    }

    /// Takes the import of a requested item on; `false` when another node
    /// did first (or it is no longer to import).
    pub async fn start_import(
        &self,
        actor: &str,
        link_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, AliexpressError> {
        let Some(link) = self.load_link_by_id(link_id).await? else {
            return Ok(false);
        };
        if link.status != LinkStatus::Importing || link.started {
            return Ok(false);
        }
        let written = link
            .write()?
            .routing_key_opt(routing_key)
            .event(&ImportStarted {
                started_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Records the draft an import filled.
    pub async fn record_import(
        &self,
        actor: &str,
        link_id: &str,
        product_id: &str,
        skus: Vec<SkuLink>,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let link = self.require_link(link_id).await?;
        link.write()?
            .routing_key_opt(routing_key)
            .event(&Imported {
                product_id: product_id.to_owned(),
                skus,
                imported_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Records why an import made nothing.
    pub async fn record_import_failure(
        &self,
        actor: &str,
        link_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let link = self.require_link(link_id).await?;
        link.write()?
            .routing_key_opt(routing_key)
            .event(&ImportFailed {
                reason: reason.chars().take(500).collect(),
                failed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Records a read of a linked item, written against `version` (the one
    /// the read started from): another node's read in between wins and this
    /// one is `Write(InvalidOriginalVersion)`.
    pub async fn record_sync(
        &self,
        actor: &str,
        link_id: &str,
        version: u16,
        read: SyncRead,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let mut write = evento::append(link_id);
        write
            .original_version(version)
            .routing_key_opt(routing_key)
            .event(&Synced {
                skus: read.skus,
                offline: read.offline,
                synced_by: actor.to_owned(),
            });
        if let Some(found) = read.found {
            write.event(&CombinationsFound { found });
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }

    /// Records combinations found earlier that are now variants, written
    /// against `version` like a read.
    pub async fn record_combinations_added(
        &self,
        actor: &str,
        link_id: &str,
        version: u16,
        skus: Vec<SkuLink>,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        if skus.is_empty() {
            return Ok(());
        }
        evento::append(link_id)
            .original_version(version)
            .routing_key_opt(routing_key)
            .event(&CombinationsAdded {
                skus,
                added_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Stops offering a combination found; ignored already is a no-op.
    pub async fn ignore_combination(
        &self,
        actor: &str,
        link_id: &str,
        sku_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let link = self.require_link(link_id).await?;
        if !link.found.iter().any(|sku| sku.sku_id == sku_id) {
            return Err(AliexpressError::CombinationNotFound);
        }
        if link.ignored.iter().any(|ignored| ignored == sku_id) {
            return Ok(());
        }
        link.write()?
            .routing_key_opt(routing_key)
            .event(&CombinationIgnored {
                sku_id: sku_id.to_owned(),
                ignored_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Offers an ignored combination again; one not ignored is a no-op.
    pub async fn restore_combination(
        &self,
        actor: &str,
        link_id: &str,
        sku_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let link = self.require_link(link_id).await?;
        if !link.ignored.iter().any(|ignored| ignored == sku_id) {
            return Ok(());
        }
        link.write()?
            .routing_key_opt(routing_key)
            .event(&CombinationRestored {
                sku_id: sku_id.to_owned(),
                restored_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Records why a read of a linked item failed.
    pub async fn record_sync_failure(
        &self,
        actor: &str,
        link_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let link = self.require_link(link_id).await?;
        link.write()?
            .routing_key_opt(routing_key)
            .event(&SyncFailed {
                reason: reason.chars().take(500).collect(),
                failed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// The product's own pricing; `None` follows the shop's.
    pub async fn set_pricing(
        &self,
        actor: &str,
        link_id: &str,
        pricing: Option<Pricing>,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        if pricing.as_ref().is_some_and(|pricing| !pricing.is_valid()) {
            return Err(AliexpressError::InvalidPricingRule);
        }
        let link = self.require_link(link_id).await?;
        if link.pricing == pricing {
            return Ok(());
        }
        link.write()?
            .routing_key_opt(routing_key)
            .event(&PricingSet {
                pricing,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Stops keeping the product in step; unlinked already is a no-op.
    pub async fn unlink(
        &self,
        actor: &str,
        link_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        let link = self.require_link(link_id).await?;
        if link.status == LinkStatus::Unlinked {
            return Ok(());
        }
        link.write()?
            .routing_key_opt(routing_key)
            .event(&Unlinked {
                unlinked_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

/// Takes the renewal of `company_id`'s token lapsing at `expires_at` on;
/// `false` when another node did. The claim lives in the platform's own
/// streams, next to the company, not under any tenant.
pub async fn claim_token_refresh<E: Executor>(
    executor: &E,
    actor: &str,
    company_id: &str,
    expires_at: u64,
) -> anyhow::Result<bool> {
    let claimed = evento::append(crate::value_object::token_refresh_id(
        company_id, expires_at,
    ))
    .original_version(0)
    .event(&crate::aggregator::refresh::Claimed {
        claimed_by: actor.to_owned(),
    })
    .commit(executor)
    .await;
    match claimed {
        Ok(_) => Ok(true),
        Err(WriteError::InvalidOriginalVersion) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

impl<E: Executor> Command<E> {
    /// The link of an AliExpress item, if the shop ever asked for it.
    pub async fn load_link(&self, ae_product_id: &str) -> anyhow::Result<Option<LinkState>> {
        self.load_link_by_id(&aliexpress_product_id(ae_product_id))
            .await
    }

    pub async fn load_link_by_id(&self, link_id: &str) -> anyhow::Result<Option<LinkState>> {
        link_projection()
            .load(link_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The link and the version it was read at.
    pub async fn load_link_versioned(
        &self,
        link_id: &str,
    ) -> anyhow::Result<Option<(LinkState, u16)>> {
        let Some(link) = self.load_link_by_id(link_id).await? else {
            return Ok(None);
        };
        let version = link.aggregate_version()?;
        Ok(Some((link, version)))
    }

    async fn require_link(&self, link_id: &str) -> Result<LinkState, AliexpressError> {
        self.load_link_by_id(link_id)
            .await?
            .ok_or(AliexpressError::NotFound)
    }
}
