use evento::{Executor, WriteError};
use ulid::Ulid;

use crate::aggregator::{OAuth2AccountLinked, Registered, SignedIn};
use crate::error::IdentityError;
use crate::value_object::{LoginContext, OAuth2Mode, ProviderIdentity, SignInMethod};

use super::sign_in::SignInFacts;

/// A verified OIDC callback.
#[derive(Debug, Clone)]
pub struct CompleteOAuth2 {
    pub mode: OAuth2Mode,
    /// The user the browser is signed in as, if any. For `AddToUser` the web
    /// layer must have checked it equals the user who started the flow.
    pub session_user_id: Option<String>,
    pub identity: ProviderIdentity,
    pub context: LoginContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuth2Outcome {
    pub user_id: String,
    /// A new user was registered.
    pub created: bool,
    /// The identity was linked to an existing user; no sign-in happened.
    pub linked: bool,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Decides what a verified provider identity means for the current
    /// browser, exactly as oauth2-passkey does: by mode, by whether a session
    /// exists, and by whether the identity is already registered.
    pub async fn complete_oauth2(
        &self,
        cmd: CompleteOAuth2,
        routing_key: Option<String>,
    ) -> Result<OAuth2Outcome, IdentityError> {
        let existing = self
            .load_oauth2_account(&cmd.identity.provider, &cmd.identity.provider_user_id)
            .await?;
        let facts = SignInFacts {
            method: SignInMethod::OAuth2,
            provider: Some(cmd.identity.provider.clone()),
            credential_id: None,
            context: cmd.context.clone(),
        };

        match (cmd.mode, cmd.session_user_id.as_deref(), existing) {
            (OAuth2Mode::AddToUser, Some(user_id), existing) => {
                if let Some(existing) = &existing
                    && existing.user_id != user_id
                {
                    return Err(IdentityError::AlreadyLinkedToOtherUser);
                }
                self.link_oauth2(user_id, &cmd.identity, routing_key)
                    .await?;
                Ok(OAuth2Outcome {
                    user_id: user_id.to_owned(),
                    created: false,
                    linked: true,
                })
            }
            (OAuth2Mode::Login | OAuth2Mode::CreateUserOrLogin, None, Some(existing)) => {
                self.record_sign_in(&existing.user_id, facts, routing_key)
                    .await?;
                Ok(OAuth2Outcome {
                    user_id: existing.user_id,
                    created: false,
                    linked: false,
                })
            }
            (OAuth2Mode::Login, None, None) => Err(IdentityError::NotRegistered),
            (OAuth2Mode::CreateUser | OAuth2Mode::CreateUserOrLogin, None, None) => {
                let user_id = self
                    .register_oauth2(&cmd.identity, &cmd.context, routing_key)
                    .await?;
                Ok(OAuth2Outcome {
                    user_id,
                    created: true,
                    linked: false,
                })
            }
            (OAuth2Mode::CreateUser, None, Some(_)) => Err(IdentityError::AlreadyRegistered),
            (OAuth2Mode::AddToUser, None, _) => Err(IdentityError::Unauthorized),
            (_, Some(_), _) => Err(IdentityError::UnexpectedlyAuthorized),
        }
    }
}

impl<E: Executor> super::Command<E> {
    /// Registers a user from a provider identity: the user stream first (an
    /// unreachable stub if the index race is lost), then the index entry.
    async fn register_oauth2(
        &self,
        identity: &ProviderIdentity,
        context: &LoginContext,
        routing_key: Option<String>,
    ) -> Result<String, IdentityError> {
        let user_id = Ulid::generate().to_string();
        evento::append(&user_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Registered {
                account: identity.account(),
                label: identity.label(),
                method: SignInMethod::OAuth2,
            })
            .event(&OAuth2AccountLinked {
                provider: identity.provider.clone(),
                provider_user_id: identity.provider_user_id.clone(),
                name: identity.name.clone(),
                email: identity.email.clone(),
                picture: identity.picture.clone(),
            })
            .event(&SignedIn {
                method: SignInMethod::OAuth2,
                provider: Some(identity.provider.clone()),
                credential_id: None,
                user_agent: context.user_agent.clone(),
                ip: context.ip.clone(),
            })
            .commit(&self.0)
            .await?;

        match self
            .link_oauth2_index(identity, &user_id, routing_key.clone())
            .await
        {
            Ok(()) => {}
            Err(IdentityError::AlreadyLinkedToOtherUser) => {
                tracing::warn!(
                    orphan_user_id = %user_id,
                    provider = %identity.provider,
                    "lost the registration race; the stub user is unreachable"
                );
                return Err(IdentityError::AlreadyRegistered);
            }
            Err(err) => return Err(err),
        }

        self.claim_first_user(&user_id, routing_key).await?;
        Ok(user_id)
    }

    /// Links a provider identity to an existing user: index first (the
    /// uniqueness check), then the user's own record of it.
    async fn link_oauth2(
        &self,
        user_id: &str,
        identity: &ProviderIdentity,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if self.load_user(user_id).await?.is_none() {
            return Err(IdentityError::UserNotFound);
        }
        self.link_oauth2_index(identity, user_id, routing_key.clone())
            .await?;
        let result = self
            .append_user(user_id, routing_key, |user, builder| {
                if user.has_oauth2_account(&identity.provider, &identity.provider_user_id) {
                    return Err(IdentityError::InvalidState);
                }
                builder.event(&OAuth2AccountLinked {
                    provider: identity.provider.clone(),
                    provider_user_id: identity.provider_user_id.clone(),
                    name: identity.name.clone(),
                    email: identity.email.clone(),
                    picture: identity.picture.clone(),
                });
                Ok(())
            })
            .await;
        match result {
            // Already listed on the user: idempotent.
            Ok(_) | Err(IdentityError::InvalidState) => Ok(()),
            Err(IdentityError::Write(WriteError::InvalidOriginalVersion)) => {
                Err(IdentityError::InvalidState)
            }
            Err(err) => Err(err),
        }
    }
}
