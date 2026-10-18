//! Write side: the link between an AliExpress item and the catalog product
//! it filled, and its history of reads.

use std::ops::Deref;
use std::time::{SystemTime, UNIX_EPOCH};

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_tenancy::PricingRule;

use crate::aggregator::{
    ImportFailed, ImportRequested, ImportStarted, Imported, PricingSet, SkuLink, SyncFailed,
    Synced, Unlinked,
};
use crate::error::AliexpressError;
use crate::value_object::{IMPORT_STALE_SECS, aliexpress_product_id};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
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
    /// The product's own rule; `None` follows the shop's.
    pub pricing: Option<PricingRule>,
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
    link.pricing = event.data.pricing;
    link.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_unlinked(event: Event<Unlinked>, link: &mut LinkState) -> anyhow::Result<()> {
    link.status = LinkStatus::Unlinked;
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
                    .commit(&self.0)
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
                    .commit(&self.0)
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
            .commit(&self.0)
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
            .commit(&self.0)
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
            .commit(&self.0)
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
        skus: Vec<SkuLink>,
        offline: bool,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        evento::append(link_id)
            .original_version(version)
            .routing_key_opt(routing_key)
            .event(&Synced {
                skus,
                offline,
                synced_by: actor.to_owned(),
            })
            .commit(&self.0)
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
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// The product's own price rule; `None` follows the shop's.
    pub async fn set_pricing(
        &self,
        actor: &str,
        link_id: &str,
        pricing: Option<PricingRule>,
        routing_key: Option<String>,
    ) -> Result<(), AliexpressError> {
        if pricing.is_some_and(|rule| !rule.is_valid()) {
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
            .commit(&self.0)
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
            .commit(&self.0)
            .await?;
        Ok(())
    }
}

impl<E: Executor> Command<E> {
    /// The link of an AliExpress item, if the shop ever asked for it.
    pub async fn load_link(&self, ae_product_id: &str) -> anyhow::Result<Option<LinkState>> {
        self.load_link_by_id(&aliexpress_product_id(ae_product_id))
            .await
    }

    pub async fn load_link_by_id(&self, link_id: &str) -> anyhow::Result<Option<LinkState>> {
        link_projection().load(link_id).execute(&self.0).await
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
