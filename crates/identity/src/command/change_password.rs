use evento::Executor;

use crate::aggregator::{PasswordChanged, SessionsRevoked};
use crate::error::IdentityError;
use crate::password::verify_password;
use crate::value_object::session_id;

/// A new password chosen from the account page. Knowing the current one is
/// the proof; the web layer hashes the new one (see [`crate::hash_password`]).
#[derive(Clone)]
pub struct ChangePassword {
    pub user_id: String,
    pub current_password: String,
    pub new_password_hash: String,
    /// The SHA-256 of the asking browser's session token: that session stays
    /// signed in, every other session of the user ends.
    pub session_token_hash: Option<[u8; 32]>,
}

impl std::fmt::Debug for ChangePassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangePassword")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Replaces the password of a user who typed the current one. A user
    /// without the e-mail + password method has nothing to change. The user's
    /// other sessions end in the same write, as after a reset.
    pub async fn change_password(
        &self,
        cmd: ChangePassword,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let Some(user) = self.load_user(&cmd.user_id).await? else {
            return Err(IdentityError::UserNotFound);
        };
        let Some(method) = &user.password else {
            return Err(IdentityError::NotRegistered);
        };
        if !verify_password(method.password_hash.clone(), cmd.current_password).await? {
            return Err(IdentityError::WrongPassword);
        }
        let verified_hash = method.password_hash.clone();
        let except_session = cmd.session_token_hash.as_ref().map(session_id);
        self.append_user(&cmd.user_id, routing_key, |user, builder| {
            // The password moved under us (another tab): the proof is stale.
            match &user.password {
                Some(method) if method.password_hash == verified_hash => {}
                _ => return Err(IdentityError::WrongPassword),
            }
            builder.event(&PasswordChanged {
                password_hash: cmd.new_password_hash.clone(),
            });
            builder.event(&SessionsRevoked {
                except_session: except_session.clone(),
            });
            Ok(())
        })
        .await?;
        Ok(())
    }
}
