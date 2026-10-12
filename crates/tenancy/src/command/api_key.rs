//! Store API keys: a member with `api-keys:manage` creates one and gets the
//! token once; a storefront presenting it is served the tenant's event
//! store. The stream is keyed by the hash of the token, so the store never
//! holds anything a connection could be made from.

use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::api_key::{Created, Revoked};
use crate::error::TenancyError;
use crate::value_object::{
    PERMISSION_API_KEYS_MANAGE, api_key_hint, api_key_id, generate_api_key_token,
    normalize_api_key_label,
};

use super::{ApiKeyState, TenantState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateApiKey {
    pub tenant_id: String,
    pub label: String,
}

/// A created key with its token. The plaintext token exists only in this
/// value (and wherever the person who created it pastes it).
#[derive(Clone, PartialEq, Eq)]
pub struct CreatedApiKey {
    pub token: String,
    pub key: ApiKeyState,
}

impl std::fmt::Debug for CreatedApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedApiKey")
            .field("token", &"******")
            .field("key", &self.key)
            .finish()
    }
}

/// A key that opened a connection, and the tenant it belongs to.
#[derive(Debug)]
pub struct ApiKeyAccess {
    pub key: ApiKeyState,
    pub tenant: TenantState,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records a key and returns its token. Needs `api-keys:manage`.
    pub async fn create_api_key(
        &self,
        actor_user_id: &str,
        cmd: CreateApiKey,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<CreatedApiKey, TenancyError> {
        let label = normalize_api_key_label(&cmd.label)?;
        let tenant = self.require_active(&cmd.tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_API_KEYS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }

        let token = generate_api_key_token();
        let id = api_key_id(&token);
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                tenant_id: tenant.id,
                label,
                hint: api_key_hint(&token),
                created_by: actor_user_id.to_owned(),
                created_at: now,
            })
            .commit(&self.0)
            .await
            .map_err(|err| match err {
                // 256 random bits collided: not a user's doing.
                WriteError::InvalidOriginalVersion => {
                    TenancyError::Other(anyhow::anyhow!("API key token collision"))
                }
                err => err.into(),
            })?;
        let key = self
            .load_api_key(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("API key {id} vanished after creation"))?;
        Ok(CreatedApiKey { token, key })
    }

    /// Revokes a key for good. Needs `api-keys:manage` on its tenant; a key
    /// already revoked, or unknown, is `ApiKeyInvalid`.
    pub async fn revoke_api_key(
        &self,
        actor_user_id: &str,
        key_id: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let key = self
            .load_api_key(key_id)
            .await?
            .ok_or(TenancyError::ApiKeyInvalid)?;
        let tenant = self.require_active(&key.tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_API_KEYS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if !key.is_active() {
            return Err(TenancyError::ApiKeyInvalid);
        }
        key.write()?
            .routing_key_opt(routing_key)
            .event(&Revoked {
                revoked_by: actor_user_id.to_owned(),
                revoked_at: now,
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// The key and tenant a presented token opens: `ApiKeyInvalid` for a
    /// token nobody issued or one that was revoked, `Suspended` while the
    /// tenant is offline (its store is not served).
    pub async fn authenticate_api_key(&self, token: &str) -> Result<ApiKeyAccess, TenancyError> {
        let key = self
            .load_api_key_by_token(token)
            .await?
            .filter(ApiKeyState::is_active)
            .ok_or(TenancyError::ApiKeyInvalid)?;
        let tenant = self.require_active(&key.tenant_id).await?;
        Ok(ApiKeyAccess { key, tenant })
    }
}
