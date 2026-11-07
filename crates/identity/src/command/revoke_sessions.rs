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
    /// again right away. A session a newer revocation already ended cannot
    /// spare itself (a stolen one asking just after a reset): `Unauthorized`.
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
        let spared = match except_token_hash {
            Some(token_hash) => self
                .load_session(token_hash)
                .await?
                .filter(|session| session.user_id == user_id),
            None => None,
        };
        self.append_user(user_id, routing_key, |user, builder| {
            // Checked against the user as written, not as the page saw it.
            if let (Some(revocation), Some(spared)) = (&user.sessions_revoked, &spared)
                && revocation.ended_before(spared)
            {
                return Err(IdentityError::Unauthorized);
            }
            builder.event(&SessionsRevoked {
                except_session: except_session.clone(),
            });
            Ok(())
        })
        .await?;
        Ok(())
    }
}
