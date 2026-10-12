//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a tenant stream is a handful of events.

mod create;
mod rename;
mod slug_index;
mod suspend;

use std::collections::BTreeMap;
use std::ops::Deref;

pub use create::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{
    Claimed, Created, MemberGranted, MemberRevoked, MemberRoleChanged, Released, Renamed, Resumed,
    Suspended,
};
use crate::value_object::{ROLE_OWNER, tenant_slug_id};

/// The command gateway. Cheap to clone when `E` is (the dashboard's executor
/// is an `Arc` wrapper).
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// =============================================================================
// Tenant
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/TenantState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TenantState {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub created_by: String,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// The reason given when the tenant was suspended; `None` while active.
    pub suspended: Option<String>,
    /// `user id → role`, sorted so pages are stable.
    pub members: BTreeMap<String, String>,
}

impl TenantState {
    pub fn is_suspended(&self) -> bool {
        self.suspended.is_some()
    }

    pub fn role_of(&self, user_id: &str) -> Option<&str> {
        self.members.get(user_id).map(String::as_str)
    }

    pub fn is_owner(&self, user_id: &str) -> bool {
        self.role_of(user_id) == Some(ROLE_OWNER)
    }
}

pub fn tenant_projection<E: Executor>() -> Projection<E, TenantState> {
    Projection::new::<crate::aggregator::Tenant>()
        .handler(on_created())
        .handler(on_renamed())
        .handler(on_suspended())
        .handler(on_resumed())
        .handler(on_member_granted())
        .handler(on_member_role_changed())
        .handler(on_member_revoked())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.id = event.aggregate_id.to_owned();
    tenant.slug = event.data.slug.to_owned();
    tenant.name = event.data.name.to_owned();
    tenant.created_by = event.data.created_by.to_owned();
    tenant.created_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renamed(event: Event<Renamed>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.name = event.data.name.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_suspended(event: Event<Suspended>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.suspended = Some(event.data.reason.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_resumed(_event: Event<Resumed>, tenant: &mut TenantState) -> anyhow::Result<()> {
    tenant.suspended = None;
    Ok(())
}

#[evento::handler]
async fn on_member_granted(
    event: Event<MemberGranted>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant
        .members
        .insert(event.data.user_id.to_owned(), event.data.role.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_member_role_changed(
    event: Event<MemberRoleChanged>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    if let Some(role) = tenant.members.get_mut(&event.data.user_id) {
        *role = event.data.role.to_owned();
    }
    Ok(())
}

#[evento::handler]
async fn on_member_revoked(
    event: Event<MemberRevoked>,
    tenant: &mut TenantState,
) -> anyhow::Result<()> {
    tenant.members.remove(&event.data.user_id);
    Ok(())
}

// =============================================================================
// TenantSlug (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-tenancy/TenantSlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TenantSlugState {
    pub id: String,
    pub tenant_id: String,
    pub released: bool,
}

pub fn tenant_slug_projection<E: Executor>() -> Projection<E, TenantSlugState> {
    Projection::new::<crate::aggregator::TenantSlug>()
        .handler(on_slug_claimed())
        .handler(on_slug_released())
        .strict()
}

#[evento::handler]
async fn on_slug_claimed(event: Event<Claimed>, index: &mut TenantSlugState) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.tenant_id = event.data.tenant_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_slug_released(
    _event: Event<Released>,
    index: &mut TenantSlugState,
) -> anyhow::Result<()> {
    index.released = true;
    Ok(())
}

// =============================================================================
// Loads
// =============================================================================

impl<E: Executor> Command<E> {
    pub async fn load_tenant(&self, tenant_id: &str) -> anyhow::Result<Option<TenantState>> {
        tenant_projection().load(tenant_id).execute(&self.0).await
    }

    /// The tenant a (normalized) slug points at, if any.
    pub async fn load_tenant_by_slug(&self, slug: &str) -> anyhow::Result<Option<TenantState>> {
        let Some(index) = self.load_slug_index(slug).await? else {
            return Ok(None);
        };
        self.load_tenant(&index.tenant_id).await
    }

    /// The live index entry of a slug (released ones are `None`).
    pub async fn load_slug_index(&self, slug: &str) -> anyhow::Result<Option<TenantSlugState>> {
        Ok(self
            .load_slug_index_any(slug)
            .await?
            .filter(|index| !index.released))
    }

    /// The index entry including released ones (needed to re-claim).
    pub(crate) async fn load_slug_index_any(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<TenantSlugState>> {
        tenant_slug_projection()
            .load(tenant_slug_id(slug))
            .execute(&self.0)
            .await
    }
}
