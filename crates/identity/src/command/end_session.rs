use evento::Executor;
use evento::ProjectionAggregate;

use crate::aggregator::{Ended, SignedOut};
use crate::error::IdentityError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Ends a session. Returns the user it belonged to, or `None` when it was
    /// unknown or already ended.
    pub async fn end_session(
        &self,
        token_hash: &[u8; 32],
        routing_key: Option<String>,
    ) -> Result<Option<String>, IdentityError> {
        let Some(session) = self.load_session(token_hash).await? else {
            return Ok(None);
        };
        if session.ended {
            return Ok(None);
        }
        session
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&Ended)
            .commit(&self.0)
            .await?;

        // History only; a deleted user is still signed out.
        match self
            .append_user(&session.user_id, routing_key, |_, builder| {
                builder.event(&SignedOut);
                Ok(())
            })
            .await
        {
            Ok(_) | Err(IdentityError::UserNotFound) => {}
            Err(err) => return Err(err),
        }
        Ok(Some(session.user_id))
    }
}
