use evento::Executor;
use ulid::Ulid;

use crate::aggregator::{Created, MemberGranted};
use crate::error::TenancyError;
use crate::value_object::{ROLE_OWNER, normalize_name, normalize_slug};

use super::{Command, TenantState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTenant {
    /// The URL handle; lowercased and validated.
    pub slug: String,
    pub name: String,
    /// The signed-in user creating the tenant; becomes its owner.
    pub owner_user_id: String,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Claims the slug, then creates the tenant with its owner in one commit.
    /// A tenant stream that cannot be created after the claim gives the slug
    /// back (best effort: a failure there only leaves the slug reserved).
    pub async fn create_tenant(
        &self,
        cmd: CreateTenant,
        routing_key: Option<String>,
    ) -> Result<TenantState, TenancyError> {
        let slug = normalize_slug(&cmd.slug)?;
        let name = normalize_name(&cmd.name)?;
        let tenant_id = Ulid::generate().to_string();

        self.claim_slug(&slug, &tenant_id, routing_key.clone())
            .await?;

        let created = evento::append(&tenant_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Created {
                slug: slug.clone(),
                name,
                created_by: cmd.owner_user_id.clone(),
            })
            .event(&MemberGranted {
                user_id: cmd.owner_user_id,
                role: ROLE_OWNER.to_owned(),
            })
            .commit(&self.0)
            .await;
        if let Err(err) = created {
            if let Err(release) = self.release_slug(&slug, &tenant_id, routing_key).await {
                tracing::warn!(%slug, %release, "cannot give the slug back after a failed creation");
            }
            return Err(err.into());
        }

        self.load_tenant(&tenant_id)
            .await?
            .ok_or(TenancyError::NotFound)
    }
}

impl<E: Executor> Command<E> {
    /// The tenant, or `NotFound`.
    pub(crate) async fn require_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<TenantState, TenancyError> {
        self.load_tenant(tenant_id)
            .await?
            .ok_or(TenancyError::NotFound)
    }
}
