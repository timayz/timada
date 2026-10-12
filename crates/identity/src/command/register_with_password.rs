use evento::Executor;
use ulid::Ulid;

use crate::aggregator::{EmailRegistered, SignedInWithPassword};
use crate::error::IdentityError;
use crate::value_object::{ChallengePurpose, LoginContext, normalize_email};

/// A sign-up form whose address was proven with a mailed code. The web layer
/// hashes the password (see [`crate::hash_password`]) when the form is
/// submitted and carries the hash until the code arrives.
#[derive(Clone)]
pub struct RegisterWithPassword {
    pub email: String,
    pub code: String,
    pub password_hash: String,
    pub context: LoginContext,
}

impl std::fmt::Debug for RegisterWithPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterWithPassword")
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordRegistration {
    pub user_id: String,
    /// Display name of the new user (their address).
    pub label: String,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Burns the registration code, then creates the user: the user stream
    /// first (an unreachable stub if the index race is lost), then the index
    /// entry, then the first-user claim. The new user is signed in.
    pub async fn register_with_password(
        &self,
        cmd: RegisterWithPassword,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<PasswordRegistration, IdentityError> {
        let email = normalize_email(&cmd.email)?;
        self.consume_email_code(
            &email,
            ChallengePurpose::Register,
            &cmd.code,
            now,
            routing_key.clone(),
        )
        .await?;
        if let Some(owner) = self.load_email_account(&email).await?
            && self.load_user(&owner.user_id).await?.is_some()
        {
            return Err(IdentityError::AlreadyRegistered);
        }

        let user_id = Ulid::generate().to_string();
        evento::append(&user_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&EmailRegistered {
                email: email.clone(),
                label: email.clone(),
                password_hash: cmd.password_hash,
            })
            .event(&SignedInWithPassword {
                user_agent: cmd.context.user_agent,
                ip: cmd.context.ip,
            })
            .commit(&self.0)
            .await?;

        match self
            .claim_email_index(&email, &user_id, routing_key.clone())
            .await
        {
            Ok(()) => {}
            Err(IdentityError::AlreadyRegistered) => {
                tracing::warn!(
                    orphan_user_id = %user_id,
                    "lost the registration race; the stub user is unreachable"
                );
                return Err(IdentityError::AlreadyRegistered);
            }
            Err(err) => return Err(err),
        }

        self.claim_first_user(&user_id, routing_key).await?;
        Ok(PasswordRegistration {
            user_id,
            label: email,
        })
    }
}
