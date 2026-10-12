use evento::Executor;

use crate::aggregator::SessionsRevoked;
use crate::error::IdentityError;
use crate::value_object::session_id;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Voids every session of `user_id` started up to now, except the one
    /// presenting `except_token_hash` (the browser asking to sign out
    /// everywhere else). `actor_id` is the user themself or an administrator.
    /// Sessions started afterwards are unaffected, so the user can sign in
    /// again right away.
    pub async fn revoke_sessions(
        &self,
        actor_id: &str,
        user_id: &str,
        except_token_hash: Option<&[u8; 32]>,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if actor_id != user_id {
            let actor = self.load_user(actor_id).await?;
            if !actor.is_some_and(|actor| actor.is_admin) {
                return Err(IdentityError::Forbidden);
            }
        }
        let except_session = except_token_hash.map(session_id);
        self.append_user(user_id, routing_key, |_, builder| {
            builder.event(&SessionsRevoked {
                except_session: except_session.clone(),
            });
            Ok(())
        })
        .await?;
        Ok(())
    }
}
