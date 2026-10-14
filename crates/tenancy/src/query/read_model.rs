//! The subscription that keeps the SQL read models current.
//!
//! Each handler reloads the strict write model of the stream the event belongs
//! to and rewrites its rows, so delivery may repeat (the subscription is
//! at-least-once) without the rows drifting.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::api_key::ApiKey;
use crate::aggregator::{Invitation, Tenant};
use crate::command::{
    ApiKeyState, InvitationState, TenantState, api_key_projection, invitation_projection,
    tenant_projection,
};
use crate::value_object::group_of;

/// The cursor identity of the read-model subscription. One per node.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-tenancy/read-models";

/// Builds the subscription over every routing key (the catalog is
/// platform-wide). `pool` must be allowed to write; start it with
/// `.start(&executor)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .handler(on_tenant_event())
        .handler(on_invitation_event())
        .handler(on_api_key_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_tenant_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Tenant>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(tenant) = tenant_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_tenant(&pool, &tenant).await
}

async fn upsert_tenant(pool: &SqlitePool, tenant: &TenantState) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let was: Option<(String, String)> =
        sqlx::query_as("SELECT slug, name FROM tenancy_tenants WHERE id = ?1")
            .bind(&tenant.id)
            .fetch_optional(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO tenancy_tenants (id, slug, name, created_by, created_at, suspended,
            suspended_reason, group_id, member_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name,
            suspended = excluded.suspended, suspended_reason = excluded.suspended_reason,
            member_count = excluded.member_count",
    )
    .bind(&tenant.id)
    .bind(&tenant.slug)
    .bind(&tenant.name)
    .bind(&tenant.created_by)
    .bind(tenant.created_at as i64)
    .bind(tenant.is_suspended())
    .bind(tenant.suspended.as_deref())
    .bind(group_of(&tenant.id) as i64)
    .bind(tenant.members.len() as i64)
    .execute(&mut *tx)
    .await?;
    // A new row is one more tenant to the administrator's list; a repeated
    // delivery finds the row there and counts nothing.
    if was.is_none() {
        sqlx::query("UPDATE tenancy_totals SET tenants = tenants + 1 WHERE id = 1")
            .execute(&mut *tx)
            .await?;
    }
    // What the administrator's search finds it by, rewritten only when that
    // changed.
    if was
        .as_ref()
        .is_none_or(|(slug, name)| *slug != tenant.slug || *name != tenant.name)
    {
        sqlx::query(
            "DELETE FROM tenancy_tenants_fts
             WHERE rowid = (SELECT rowid FROM tenancy_tenants WHERE id = ?1)",
        )
        .bind(&tenant.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO tenancy_tenants_fts (rowid, slug, name)
             SELECT rowid, slug, name FROM tenancy_tenants WHERE id = ?1",
        )
        .bind(&tenant.id)
        .execute(&mut *tx)
        .await?;
    }

    // The member rows are rewritten from the state: a revoked member goes
    // away, a changed role is replaced.
    sqlx::query("DELETE FROM tenancy_members WHERE tenant_id = ?1")
        .bind(&tenant.id)
        .execute(&mut *tx)
        .await?;
    for (user_id, role) in &tenant.members {
        sqlx::query("INSERT INTO tenancy_members (tenant_id, user_id, role) VALUES (?1, ?2, ?3)")
            .bind(&tenant.id)
            .bind(user_id)
            .bind(role)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_invitation_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Invitation>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(invitation) = invitation_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_invitation(&pool, &invitation).await
}

async fn upsert_invitation(pool: &SqlitePool, invitation: &InvitationState) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO tenancy_invitations (id, tenant_id, email, role, invited_by, sent_at,
            expires_at, accepted_by, accepted_at, revoked)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (id) DO UPDATE SET
            accepted_by = excluded.accepted_by, accepted_at = excluded.accepted_at,
            revoked = excluded.revoked",
    )
    .bind(&invitation.id)
    .bind(&invitation.tenant_id)
    .bind(&invitation.email)
    .bind(&invitation.role)
    .bind(&invitation.invited_by)
    .bind(invitation.sent_at as i64)
    .bind(invitation.expires_at as i64)
    .bind(invitation.accepted_by.as_deref())
    .bind(invitation.accepted_at.map(|at| at as i64))
    .bind(invitation.revoked)
    .execute(pool)
    .await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_api_key_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ApiKey>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(key) = api_key_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_api_key(&pool, &key).await
}

async fn upsert_api_key(pool: &SqlitePool, key: &ApiKeyState) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO tenancy_api_keys (id, tenant_id, label, hint, created_by, created_at,
            revoked_by, revoked_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
            revoked_by = excluded.revoked_by, revoked_at = excluded.revoked_at",
    )
    .bind(&key.id)
    .bind(&key.tenant_id)
    .bind(&key.label)
    .bind(&key.hint)
    .bind(&key.created_by)
    .bind(key.created_at as i64)
    .bind(key.revoked_by.as_deref())
    .bind(key.revoked_at.map(|at| at as i64))
    .execute(pool)
    .await?;
    Ok(())
}
