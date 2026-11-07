use evento::Executor;

use crate::aggregator::{PasswordChanged, SessionsRevoked, SignedInWithPassword};
use crate::error::IdentityError;
use crate::value_object::{ChallengePurpose, LoginContext, normalize_email};

/// A new password for an address proven with a mailed code.
#[derive(Clone)]
pub struct ResetPassword {
    pub email: String,
    pub code: String,
    pub password_hash: String,
    pub context: LoginContext,
}

impl std::fmt::Debug for ResetPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResetPassword")
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordReset {
    pub user_id: String,
    pub label: String,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Burns the reset code, replaces the password and signs the user in.
    /// Every session of the user ends in the same write, so whoever held the
    /// old password is out the moment it stops working. The web layer starts
    /// the new session afterwards; one started within the reset's own
    /// millisecond counts as revoked, so it is spared by name
    /// ([`Self::revoke_sessions`]).
    pub async fn reset_password(
        &self,
        cmd: ResetPassword,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<PasswordReset, IdentityError> {
        let email = normalize_email(&cmd.email)?;
        self.consume_email_code(
            &email,
            ChallengePurpose::ResetPassword,
            &cmd.code,
            now,
            routing_key.clone(),
        )
        .await?;
        let Some(owner) = self.email_user(&email).await? else {
            return Err(IdentityError::NotRegistered);
        };
        let user = self
            .append_user(&owner.id, routing_key.clone(), |user, builder| {
                if user.password.is_none() {
                    return Err(IdentityError::NotRegistered);
                }
                builder.event(&PasswordChanged {
                    password_hash: cmd.password_hash.clone(),
                });
                builder.event(&SessionsRevoked {
                    except_session: None,
                });
                builder.event(&SignedInWithPassword {
                    user_agent: cmd.context.user_agent.clone(),
                    ip: cmd.context.ip.clone(),
                });
                Ok(())
            })
            .await?;
        self.grant_admin_if_first(&user, routing_key).await?;
        Ok(PasswordReset {
            user_id: user.id,
            label: user.label,
        })
    }
}
