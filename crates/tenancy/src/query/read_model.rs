//! The subscription that keeps the read models current.
//!
//! Each handler reloads the strict write model of the stream the event belongs
//! to and rewrites its rows, so delivery may repeat (the subscription is
//! at-least-once) without the rows drifting. The rows live in the store's own
//! database and are written through it ([`Db::write`]).

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, execute, fetch_optional, search_key};

use super::api_keys::join_scopes;
use crate::aggregator::api_key::ApiKey;
use crate::aggregator::company::Company;
use crate::aggregator::{Invitation, Tenant};
use crate::command::{
    ApiKeyState, CompanyState, InvitationState, TenantState, api_key_projection,
    company_projection, invitation_projection, tenant_projection,
};

/// The cursor identity of the read-model subscription. One per node.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-tenancy/read-models";

/// Builds the subscription over every routing key (the catalog is
/// platform-wide). Start it with `.start(&executor)` and keep the handle for
/// shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_tenant_event())
        .handler(on_invitation_event())
        .handler(on_api_key_event())
        .handler(on_company_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_tenant_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Tenant>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(tenant) = tenant_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_tenant(&db, &tenant).await
}

#[evento::subscription_all]
async fn on_company_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Company>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(company) = company_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_company(&db, &company).await
}

async fn upsert_company(db: &Db, company: &CompanyState) -> anyhow::Result<()> {
    db.write(company, |tx, company| {
        Box::pin(async move {
            execute(
                tx,
                "INSERT INTO tenancy_companies (id, siren, legal_name, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET legal_name = excluded.legal_name",
                (
                    company.id.as_str(),
                    company.siren.as_str(),
                    company.name(),
                    company.created_at as i64,
                ),
            )
            .await?;
            execute(
                tx,
                "DELETE FROM tenancy_company_owners WHERE company_id = ?1",
                (company.id.as_str(),),
            )
            .await?;
            for owner in &company.owners {
                execute(
                    tx,
                    "INSERT INTO tenancy_company_owners (company_id, user_id) VALUES (?1, ?2)",
                    (company.id.as_str(), owner.as_str()),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}

async fn upsert_tenant(db: &Db, tenant: &TenantState) -> anyhow::Result<()> {
    db.write(tenant, |tx, tenant| {
        Box::pin(async move {
            let was: Option<(String,)> = fetch_optional(
                tx,
                "SELECT id FROM tenancy_tenants WHERE id = ?1",
                (tenant.id.as_str(),),
            )
            .await?;
            execute(
                tx,
                "INSERT INTO tenancy_tenants (id, slug, name, created_by, created_at, suspended,
                    suspended_reason, member_count, company_id, series_code, slug_search,
                    name_search)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (id) DO UPDATE SET
                    slug = excluded.slug, name = excluded.name,
                    suspended = excluded.suspended, suspended_reason = excluded.suspended_reason,
                    member_count = excluded.member_count, company_id = excluded.company_id,
                    series_code = excluded.series_code, slug_search = excluded.slug_search,
                    name_search = excluded.name_search",
                (
                    tenant.id.as_str(),
                    tenant.slug.as_str(),
                    tenant.name.as_str(),
                    tenant.created_by.as_str(),
                    tenant.created_at as i64,
                    tenant.is_suspended(),
                    tenant.suspended.as_deref(),
                    tenant.members.len() as i64,
                    tenant.company.as_ref().map(|link| link.company_id.as_str()),
                    tenant
                        .company
                        .as_ref()
                        .map(|link| link.series_code.as_str()),
                    search_key(&tenant.slug),
                    search_key(&tenant.name),
                ),
            )
            .await?;
            // A new row is one more tenant to the administrator's list; a
            // repeated delivery finds the row there and counts nothing.
            if was.is_none() {
                execute(
                    tx,
                    "UPDATE tenancy_totals SET tenants = tenants + 1 WHERE id = 1",
                    (),
                )
                .await?;
            }
            // The member rows are rewritten from the state: a revoked member
            // goes away, a changed role is replaced.
            execute(
                tx,
                "DELETE FROM tenancy_members WHERE tenant_id = ?1",
                (tenant.id.as_str(),),
            )
            .await?;
            for (user_id, role) in &tenant.members {
                execute(
                    tx,
                    "INSERT INTO tenancy_members (tenant_id, user_id, role) VALUES (?1, ?2, ?3)",
                    (tenant.id.as_str(), user_id.as_str(), role.as_str()),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}

#[evento::subscription_all]
async fn on_invitation_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Invitation>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(invitation) = invitation_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_invitation(&db, &invitation).await
}

async fn upsert_invitation(db: &Db, invitation: &InvitationState) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO tenancy_invitations (id, tenant_id, email, role, invited_by, sent_at,
            expires_at, accepted_by, accepted_at, revoked)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (id) DO UPDATE SET
            accepted_by = excluded.accepted_by, accepted_at = excluded.accepted_at,
            revoked = excluded.revoked",
        (
            invitation.id.as_str(),
            invitation.tenant_id.as_str(),
            invitation.email.as_str(),
            invitation.role.as_str(),
            invitation.invited_by.as_str(),
            invitation.sent_at as i64,
            invitation.expires_at as i64,
            invitation.accepted_by.as_deref(),
            invitation.accepted_at.map(|at| at as i64),
            invitation.revoked,
        ),
    )
    .await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_api_key_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ApiKey>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(key) = api_key_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_api_key(&db, &key).await
}

async fn upsert_api_key(db: &Db, key: &ApiKeyState) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO tenancy_api_keys (id, tenant_id, label, hint, created_by, created_at,
            revoked_by, revoked_at, scopes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET
            revoked_by = excluded.revoked_by, revoked_at = excluded.revoked_at,
            scopes = excluded.scopes",
        (
            key.id.as_str(),
            key.tenant_id.as_str(),
            key.label.as_str(),
            key.hint.as_str(),
            key.created_by.as_str(),
            key.created_at as i64,
            key.revoked_by.as_deref(),
            key.revoked_at.map(|at| at as i64),
            join_scopes(&key.scopes),
        ),
    )
    .await?;
    Ok(())
}
