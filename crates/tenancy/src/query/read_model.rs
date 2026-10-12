//! The subscription that keeps the SQL read models current.
//!
//! The handler reloads the strict write model of the tenant the event belongs
//! to and rewrites its rows, so delivery may repeat (the subscription is
//! at-least-once) without the rows drifting.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::Tenant;
use crate::command::{TenantState, tenant_projection};
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
