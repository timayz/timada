//! The members history of a tenant: who was invited, who joined, moved or
//! left, by whom and when.
//!
//! One row per event, unique by the event id, so a repeated delivery inserts
//! nothing; rows are numbered in the order the log delivers them, which is
//! the order the page lists them in. The subscription has its own cursor:
//! started on an existing log, it writes the whole history. The account
//! behind a member change is the commit's `requested_by`; invitation events
//! name it themselves.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, FromRow};

use crate::aggregator::{Accepted, MemberGranted, MemberRevoked, MemberRoleChanged, Revoked, Sent};
use crate::command::invitation_projection;

/// The cursor identity of the members history subscription. One per node.
pub const MEMBER_AUDIT_SUBSCRIPTION: &str = "timada-tenancy/member-audit";

/// A member was added: directly by `actor`, or by accepting an invitation
/// (`actor == user_id`), or by creating the tenant (as `owner`).
pub const AUDIT_GRANTED: &str = "granted";
pub const AUDIT_ROLE_CHANGED: &str = "role_changed";
/// A member was removed by `actor`, or left (`actor == user_id`).
pub const AUDIT_REVOKED: &str = "revoked";
pub const AUDIT_INVITED: &str = "invited";
pub const AUDIT_INVITATION_ACCEPTED: &str = "invitation_accepted";
pub const AUDIT_INVITATION_REVOKED: &str = "invitation_revoked";

/// Builds the subscription over every routing key. Start it with
/// `.start(&executor)` and keep the handle for shutdown.
pub fn member_audit_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(MEMBER_AUDIT_SUBSCRIPTION)
        .data(db)
        .handler(on_member_granted())
        .handler(on_member_role_changed())
        .handler(on_member_revoked())
        .handler(on_invitation_sent())
        .handler(on_invitation_accepted())
        .handler(on_invitation_revoked())
        .any_routing_key()
}

/// One row of `tenancy_member_audit`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct MemberAuditRow {
    /// Delivery order; what a page of history continues from.
    pub seq: i64,
    pub event_id: String,
    pub tenant_id: String,
    /// Unix seconds.
    pub at: i64,
    /// One of the `AUDIT_*` constants.
    pub action: String,
    /// The account that did it.
    pub actor: String,
    /// The member concerned (member changes, accepted invitations).
    pub user_id: Option<String>,
    /// The invited address (invitations).
    pub email: Option<String>,
    pub role: Option<String>,
}

/// The history of a tenant, newest first: `limit` rows older than the row
/// numbered `before` (its [`MemberAuditRow::seq`]), or from the newest.
pub async fn member_audit_of_tenant(
    db: &Db,
    tenant_id: &str,
    before: Option<i64>,
    limit: i64,
) -> anyhow::Result<Vec<MemberAuditRow>> {
    db.fetch_all(
        "SELECT seq, event_id, tenant_id, at, action, actor, user_id, email, role
         FROM tenancy_member_audit
         WHERE tenant_id = ?1 AND (?2 IS NULL OR seq < ?2)
         ORDER BY seq DESC
         LIMIT ?3",
        (tenant_id, before, limit),
    )
    .await
}

struct Entry<'a> {
    tenant_id: &'a str,
    action: &'static str,
    actor: &'a str,
    user_id: Option<&'a str>,
    email: Option<&'a str>,
    role: Option<&'a str>,
}

async fn record(db: &Db, event: &evento::Event, entry: Entry<'_>) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO tenancy_member_audit (event_id, tenant_id, at, action, actor, user_id,
            email, role)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (event_id) DO NOTHING",
        (
            event.id.to_string(),
            entry.tenant_id,
            event.timestamp as i64,
            entry.action,
            entry.actor,
            entry.user_id,
            entry.email,
            entry.role,
        ),
    )
    .await?;
    Ok(())
}

#[evento::subscription]
async fn on_member_granted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MemberGranted>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let actor = event.metadata.requested_by()?;
    let entry = Entry {
        tenant_id: &event.aggregate_id,
        action: AUDIT_GRANTED,
        actor: &actor,
        user_id: Some(&event.data.user_id),
        email: None,
        role: Some(&event.data.role),
    };
    record(&db, &event, entry).await
}

#[evento::subscription]
async fn on_member_role_changed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MemberRoleChanged>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let actor = event.metadata.requested_by()?;
    let entry = Entry {
        tenant_id: &event.aggregate_id,
        action: AUDIT_ROLE_CHANGED,
        actor: &actor,
        user_id: Some(&event.data.user_id),
        email: None,
        role: Some(&event.data.role),
    };
    record(&db, &event, entry).await
}

#[evento::subscription]
async fn on_member_revoked<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MemberRevoked>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let actor = event.metadata.requested_by()?;
    let entry = Entry {
        tenant_id: &event.aggregate_id,
        action: AUDIT_REVOKED,
        actor: &actor,
        user_id: Some(&event.data.user_id),
        email: None,
        role: None,
    };
    record(&db, &event, entry).await
}

#[evento::subscription]
async fn on_invitation_sent<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Sent>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let entry = Entry {
        tenant_id: &event.data.tenant_id,
        action: AUDIT_INVITED,
        actor: &event.data.invited_by,
        user_id: None,
        email: Some(&event.data.email),
        role: Some(&event.data.role),
    };
    record(&db, &event, entry).await
}

#[evento::subscription]
async fn on_invitation_accepted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Accepted>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(invitation) = invitation_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let entry = Entry {
        tenant_id: &invitation.tenant_id,
        action: AUDIT_INVITATION_ACCEPTED,
        actor: &event.data.user_id,
        user_id: Some(&event.data.user_id),
        email: Some(&invitation.email),
        role: Some(&invitation.role),
    };
    record(&db, &event, entry).await
}

#[evento::subscription]
async fn on_invitation_revoked<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Revoked>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(invitation) = invitation_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let entry = Entry {
        tenant_id: &invitation.tenant_id,
        action: AUDIT_INVITATION_REVOKED,
        actor: &event.data.revoked_by,
        user_id: None,
        email: Some(&invitation.email),
        role: Some(&invitation.role),
    };
    record(&db, &event, entry).await
}
